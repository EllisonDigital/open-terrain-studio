//! Adjust nodes: Curve, Clamp, Invert, Terrace, Blur, Sharpen, Transform and
//! Warp. (Levels lives in `basic.rs`.)

use std::sync::Arc;

use terrain_core::error::Result;
use terrain_core::ops::gaussian_blur;
use terrain_core::seed::derive;
use terrain_core::{
    EvalContext, Gpu, GpuBuffer, Grid, NodeKind, NodeSchema, Outputs, ParamDef, Params, PortDef, PortType,
};

use crate::common::{direction, grid_positions, heightfield_out, lerp, seed_param, strength_param};
use crate::kernels;
use crate::noise::basis::{self, Basis};

fn adjust_schema(type_id: &str, label: &str, description: &str, params: Vec<ParamDef>) -> NodeSchema {
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: "Adjust".into(),
        description: description.into(),
        inputs: vec![PortDef::new("in", "In", PortType::Heightfield)],
        outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
        params,
        gpu: false,
    }
}

/// Run `shaders/adjust.comp` in `mode` over `input` (and `aux`), with the
/// drivable parameter `driver` (if any) in `f[0..3]`; `set` adds the rest.
fn adjust_gpu(
    ctx: &EvalContext,
    gpu: &Gpu,
    mode: u32,
    input: &GpuBuffer,
    aux: Option<&GpuBuffer>,
    driver: Option<&str>,
    set: impl FnOnce(Params) -> Params,
) -> Result<Outputs> {
    let s = ctx.spec;
    let dummy = gpu.dummy()?;
    let mut p = grid_positions(Params::grid(s).u(4, mode), s);
    let mask = match driver {
        Some(key) => {
            let field = gpu.field(ctx, key)?;
            p = p
                .u(5, field.driven())
                .f(0, field.value)
                .f(1, field.min)
                .f(2, field.max);
            gpu.field_buffer(&field)?
        }
        None => dummy.clone(),
    };
    let out = gpu.alloc(s.len())?;
    let aux = aux.unwrap_or(&dummy);
    gpu.dispatch_grid(&kernels::ADJUST, &[input, aux, &mask, &out], &set(p), s)?;
    Ok(Outputs::from([(
        "out".to_string(),
        gpu.value(PortType::Heightfield, s, out),
    )]))
}

/// The input and its Gaussian blur with radius `radius_m` (σ = radius / 2), on the GPU.
fn blurred_gpu(ctx: &EvalContext, gpu: &Gpu) -> Result<(Arc<GpuBuffer>, Arc<GpuBuffer>)> {
    let input = gpu.input(ctx, "in")?;
    let blurred = gpu.gaussian_blur(ctx.spec, &input, ctx.f64("radius_m") * 0.5)?;
    Ok((input, blurred))
}

/// A schema with a GPU kernel.
fn with_gpu(mut schema: NodeSchema) -> NodeSchema {
    schema.gpu = true;
    schema
}

/// Blend `input` towards `effect` by the (drivable) "strength" parameter.
fn blend_by_strength(ctx: &EvalContext, input: &Grid, effect: Grid) -> Result<Outputs> {
    let strength = ctx.field("strength");
    let mut out = effect;
    for (idx, v) in out.data.iter_mut().enumerate() {
        *v = lerp(input.data[idx], *v, strength.at(idx));
    }
    heightfield_out(out)
}

// ---- Curve ------------------------------------------------------------------

/// Reshape heights with a response curve.
pub struct CurveNode {
    schema: NodeSchema,
}

impl Default for CurveNode {
    fn default() -> Self {
        Self {
            schema: adjust_schema(
                "adjust.curve",
                "Curve",
                "Reshape heights with a curve. Left = the world's lowest height, right = its highest; \
                 pull the curve up to raise those heights. Heights outside the world range are clamped.",
                vec![ParamDef::curve("curve", "Curve"), strength_param()],
            ),
        }
    }
}

impl NodeKind for CurveNode {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let curve = ctx.curve("curve");
        let world = ctx.world;
        let effect = input.map(|h| {
            let t = world.normalise(h).clamp(0.0, 1.0) as f64;
            world.denormalise(curve.eval(t) as f32)
        });
        blend_by_strength(ctx, input, effect)
    }
}

// ---- Clamp ------------------------------------------------------------------

/// Limit heights to a range.
pub struct Clamp {
    schema: NodeSchema,
}

impl Default for Clamp {
    fn default() -> Self {
        Self {
            schema: adjust_schema(
                "adjust.clamp",
                "Clamp",
                "Cut off heights below Low and above High, leaving flat floors and tops.",
                vec![
                    ParamDef::metres("low_m", "Low", 0.0, -10_000.0, 20_000.0),
                    ParamDef::metres("high_m", "High", 1500.0, -10_000.0, 20_000.0),
                ],
            ),
        }
    }
}

impl NodeKind for Clamp {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let (a, b) = (ctx.f32("low_m"), ctx.f32("high_m"));
        let (lo, hi) = (a.min(b), a.max(b));
        heightfield_out(input.map(|h| h.clamp(lo, hi)))
    }
}

// ---- Invert -----------------------------------------------------------------

/// Turn the terrain upside down.
pub struct Invert {
    schema: NodeSchema,
}

impl Default for Invert {
    fn default() -> Self {
        Self {
            schema: adjust_schema(
                "adjust.invert",
                "Invert",
                "Turn the terrain upside down: peaks become pits and valleys become ridges.",
                vec![
                    ParamDef::choice(
                        "around",
                        "Flip within",
                        "world",
                        &[
                            ("world", "World height range"),
                            ("input", "Input's own range"),
                            ("pivot", "Mirror at a height"),
                        ],
                    ),
                    ParamDef::metres("pivot_m", "Pivot height", 0.0, -10_000.0, 20_000.0)
                        .describe("For \"Mirror at a height\": heights are mirrored around this one."),
                ],
            ),
        }
    }
}

impl NodeKind for Invert {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        if ctx.choice("around") == "input" {
            terrain_core::Reach::Global
        } else {
            crate::common::point_wise()
        }
    }
    fn world_spec(&self, _ctx: &EvalContext) -> Option<terrain_core::GridSpec> {
        None
    }
    fn finish_tile(&self, ctx: &EvalContext, world: &terrain_core::WorldPass) -> Result<Outputs> {
        let range = world.ranges.get("in").copied();
        self.invert(ctx, range)
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        self.invert(ctx, None)
    }
}

impl Invert {
    /// Invert over `ctx`; `range` is the input range for "Mirror the input"
    /// (the input's own when `None`).
    fn invert(&self, ctx: &EvalContext, range: Option<(f32, f32)>) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        // h' = sum - h: mirror around sum / 2.
        let sum = match ctx.choice("around").as_str() {
            "input" => {
                let (lo, hi) = range.unwrap_or_else(|| input.min_max());
                lo + hi
            }
            "pivot" => 2.0 * ctx.f32("pivot_m"),
            _ => ctx.world.height_range_m[0] + ctx.world.height_range_m[1],
        };
        heightfield_out(input.map(|h| sum - h))
    }
}

// ---- Terrace ----------------------------------------------------------------

/// Stepped terraces.
pub struct Terrace {
    schema: NodeSchema,
}

impl Default for Terrace {
    fn default() -> Self {
        Self {
            schema: adjust_schema(
                "adjust.terrace",
                "Terrace",
                "Cut the terrain into steps: flat benches with steeper risers between them.",
                vec![
                    ParamDef::metres("step_m", "Step height", 120.0, 0.1, 10_000.0)
                        .describe("Height of each step, in metres."),
                    ParamDef::float("sharpness", "Sharpness", 0.7, 0.0, 1.0)
                        .describe("0 = gentle undulation, 1 = flat benches with near-vertical risers."),
                    ParamDef::metres("offset_m", "Offset", 0.0, -10_000.0, 10_000.0)
                        .describe("Shift the steps up or down, in metres."),
                    strength_param(),
                ],
            ),
        }
    }
}

impl NodeKind for Terrace {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let step = ctx.f32("step_m");
        let offset = ctx.f32("offset_m");
        // Exponent 1 (no change) .. 12 (flat benches).
        let k = 1.0 + ctx.f32("sharpness") * 11.0;
        let effect = input.map(|h| {
            let s = (h - offset) / step;
            let f = s.floor();
            offset + (f + libm::powf(s - f, k)) * step
        });
        blend_by_strength(ctx, input, effect)
    }
}

// ---- Blur and Sharpen -------------------------------------------------------

/// Gaussian blur.
pub struct Blur {
    schema: NodeSchema,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            schema: with_gpu(adjust_schema(
                "adjust.blur",
                "Blur",
                "Smooth the terrain. Features smaller than about the radius are smoothed away.",
                vec![
                    ParamDef::metres("radius_m", "Radius", 100.0, 0.0, 100_000.0)
                        .describe("Blur radius in metres (twice the Gaussian's standard deviation)."),
                    strength_param(),
                ],
            )),
        }
    }
}

impl NodeKind for Blur {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        terrain_core::Reach::Local(crate::common::blur_reach(ctx.f64("radius_m") * 0.5))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let blurred = gaussian_blur(input, ctx.f64("radius_m") * 0.5);
        blend_by_strength(ctx, input, blurred)
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let (input, blurred) = blurred_gpu(ctx, gpu)?;
        adjust_gpu(ctx, gpu, 0, &input, Some(&blurred), Some("strength"), |p| p)
    }
}

/// Unsharp mask.
pub struct Sharpen {
    schema: NodeSchema,
}

impl Default for Sharpen {
    fn default() -> Self {
        Self {
            schema: with_gpu(adjust_schema(
                "adjust.sharpen",
                "Sharpen",
                "Exaggerate detail: bumps smaller than the radius get taller and dips deeper.",
                vec![
                    ParamDef::metres("radius_m", "Radius", 200.0, 0.0, 100_000.0)
                        .describe("Size of the detail to exaggerate, in metres."),
                    ParamDef::float("amount", "Amount", 1.0, 0.0, 10.0)
                        .describe("1 = double the detail.")
                        .drivable(),
                ],
            )),
        }
    }
}

impl NodeKind for Sharpen {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        terrain_core::Reach::Local(crate::common::blur_reach(ctx.f64("radius_m") * 0.5))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let amount = ctx.field("amount");
        let blurred = gaussian_blur(input, ctx.f64("radius_m") * 0.5);
        heightfield_out(input.map_indexed(|idx, h| h + amount.at(idx) * (h - blurred.data[idx])))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let (input, blurred) = blurred_gpu(ctx, gpu)?;
        adjust_gpu(ctx, gpu, 1, &input, Some(&blurred), Some("amount"), |p| p)
    }
}

// ---- Transform --------------------------------------------------------------

/// Move, rotate and scale.
pub struct Transform {
    schema: NodeSchema,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            schema: with_gpu(adjust_schema(
                "adjust.transform",
                "Transform",
                "Move, rotate and scale the terrain around the centre of the world. \
                 Areas brought in from outside repeat the edge.",
                vec![
                    ParamDef::metres("move_x_m", "Move X", 0.0, -1_000_000.0, 1_000_000.0),
                    ParamDef::metres("move_y_m", "Move Y", 0.0, -1_000_000.0, 1_000_000.0),
                    ParamDef::float("rotation_deg", "Rotation", 0.0, -360.0, 360.0)
                        .unit("°")
                        .describe("Clockwise in the 2D view."),
                    ParamDef::float("scale", "Scale", 1.0, 0.01, 100.0)
                        .describe("2 = features twice as large."),
                    ParamDef::float("height_scale", "Height scale", 1.0, -10.0, 10.0)
                        .describe("Multiply heights (around the world's lowest height)."),
                ],
            )),
        }
    }
}

impl NodeKind for Transform {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        // The inverse transform is affine, so the farthest any output sample
        // reads from is at a corner of the world.
        let c = ctx.world.centre();
        let (mx, my) = (ctx.f64("move_x_m"), ctx.f64("move_y_m"));
        let (cos, sin) = direction(-ctx.f64("rotation_deg"));
        let scale = ctx.f64("scale");
        let s = ctx.spec.whole();
        let mut reach: f64 = 0.0;
        for (x, y) in [
            (s.origin_m[0], s.origin_m[1]),
            (s.origin_m[0] + s.extent_m[0], s.origin_m[1]),
            (s.origin_m[0], s.origin_m[1] + s.extent_m[1]),
            (s.origin_m[0] + s.extent_m[0], s.origin_m[1] + s.extent_m[1]),
        ] {
            let (px, py) = ((x - c[0] - mx) / scale, (y - c[1] - my) / scale);
            let (sx, sy) = (px * cos - py * sin + c[0], px * sin + py * cos + c[1]);
            reach = reach.max((sx - x).hypot(sy - y));
        }
        terrain_core::Reach::Local(reach + crate::common::cell_m(ctx))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let c = ctx.world.centre();
        let (mx, my) = (ctx.f64("move_x_m"), ctx.f64("move_y_m"));
        // Inverse transform: output position -> input position.
        let (cos, sin) = direction(-ctx.f64("rotation_deg"));
        let scale = ctx.f64("scale");
        let hs = ctx.f32("height_scale");
        let h0 = ctx.world.height_range_m[0];
        heightfield_out(Grid::from_fn(ctx.spec, |x, y| {
            let (px, py) = ((x - c[0] - mx) / scale, (y - c[1] - my) / scale);
            let (sx, sy) = (px * cos - py * sin, px * sin + py * cos);
            let h = input.sample_bilinear_m(sx + c[0], sy + c[1]);
            h0 + (h - h0) * hs
        }))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let input = gpu.input(ctx, "in")?;
        let c = ctx.world.centre();
        let (cos, sin) = direction(-ctx.f64("rotation_deg"));
        adjust_gpu(ctx, gpu, 2, &input, None, None, |p| {
            p.f(3, c[0] as f32)
                .f(4, c[1] as f32)
                .f(5, ctx.f32("move_x_m"))
                .f(6, ctx.f32("move_y_m"))
                .f(7, cos as f32)
                .f(8, sin as f32)
                .f(9, ctx.f32("scale"))
                .f(10, ctx.f32("height_scale"))
                .f(11, ctx.world.height_range_m[0])
        })
    }
}

// ---- Warp -------------------------------------------------------------------

/// Push the terrain around with noise.
pub struct Warp {
    schema: NodeSchema,
}

impl Default for Warp {
    fn default() -> Self {
        Self {
            schema: with_gpu(adjust_schema(
                "adjust.warp",
                "Warp",
                "Push the terrain around with smooth noise, bending straight lines and regular shapes \
                 into natural ones.",
                vec![
                    ParamDef::metres("size_m", "Size", 1500.0, 1.0, 1_000_000.0)
                        .describe("Size of the swirls, in metres."),
                    ParamDef::metres("strength_m", "Strength", 300.0, 0.0, 100_000.0)
                        .describe("How far the terrain is pushed, in metres.")
                        .drivable(),
                    ParamDef::int("octaves", "Octaves", 4, 1, 10).describe("Detail in the swirls."),
                    seed_param(),
                ],
            )),
        }
    }
}

impl NodeKind for Warp {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        // fBm is normalised to -1..1 on each axis; a driven strength only
        // lowers it.
        let s = ctx.f64("strength_m").abs();
        terrain_core::Reach::Local(std::f64::consts::SQRT_2 * s + crate::common::cell_m(ctx))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let size = ctx.f64("size_m");
        let strength = ctx.field("strength_m");
        let octaves = ctx.i64("octaves") as u32;
        let (sa, sb) = (derive(ctx.seed, 1), derive(ctx.seed, 2));
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let s = strength.at(idx) as f64;
            let dx = basis::fbm(Basis::Perlin, x / size, y / size, sa, octaves, 2.0, 0.5);
            let dy = basis::fbm(Basis::Perlin, x / size, y / size, sb, octaves, 2.0, 0.5);
            input.sample_bilinear_m(x + dx * s, y + dy * s)
        }))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let input = gpu.input(ctx, "in")?;
        adjust_gpu(ctx, gpu, 3, &input, None, Some("strength_m"), |p| {
            p.u(6, ctx.i64("octaves") as u32)
                .u64(8, derive(ctx.seed, 1))
                .u64(10, derive(ctx.seed, 2))
                .f(3, ctx.f32("size_m"))
        })
    }
}
