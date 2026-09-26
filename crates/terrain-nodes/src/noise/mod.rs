//! Noise nodes: Perlin, Simplex, Value, fBm, Ridged, Billow, Domain warp and
//! Voronoi.
//!
//! All sizes are in metres and noise is sampled at world positions, so a
//! feature sits in the same place at any resolution (ARCHITECTURE.md §4).

pub mod basis;

use std::sync::Arc;

use terrain_core::error::Result;
use terrain_core::{
    EvalContext, Gpu, Grid, NodeKind, NodeSchema, Outputs, ParamDef, Params, PortDef, PortType, Value,
};

use crate::kernels;
use basis::Basis;

/// Parameters shared by every noise node.
fn common_params() -> Vec<ParamDef> {
    vec![
        ParamDef::metres("feature_size_m", "Feature size", 3000.0, 10.0, 100_000.0)
            .describe("Size of the largest features, in metres."),
        ParamDef::metres("height_m", "Height", 1000.0, 0.0, 20_000.0)
            .describe("Difference between the lowest and highest points, in metres.")
            .drivable(),
        ParamDef::metres("base_m", "Base", 0.0, -10_000.0, 10_000.0)
            .describe("Height of the lowest points, in metres."),
        ParamDef::metres("offset_x_m", "Offset X", 0.0, -1_000_000.0, 1_000_000.0)
            .describe("Slide the noise east/west."),
        ParamDef::metres("offset_y_m", "Offset Y", 0.0, -1_000_000.0, 1_000_000.0)
            .describe("Slide the noise north/south."),
        ParamDef::int("seed", "Seed", 0, 0, 999_999).describe("Change for a different variation."),
    ]
}

/// Sample a -1..1 noise function over the grid and map it to heights in metres.
fn noise_heightfield(ctx: &EvalContext, f: impl Fn(f64, f64) -> f64 + Sync) -> Result<Outputs> {
    let size = ctx.f64("feature_size_m");
    let height = ctx.f64("height_m");
    let field = ctx.field("height_m");
    let base = ctx.f64("base_m");
    let (ox, oy) = (ctx.f64("offset_x_m"), ctx.f64("offset_y_m"));
    let grid = Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
        let n = f((x + ox) / size, (y + oy) / size);
        // A constant height stays in f64, exactly as in v0.1.
        let h = if field.is_const() {
            height
        } else {
            field.at(idx) as f64
        };
        (base + (n * 0.5 + 0.5) * h) as f32
    });
    Ok(Outputs::from([(
        "out".to_string(),
        Value::Heightfield(Arc::new(grid)),
    )]))
}

/// The GPU version of [`noise_heightfield`]: `node` selects the function in
/// `shaders/noise.comp`, `set` adds its parameters.
fn noise_gpu(ctx: &EvalContext, gpu: &Gpu, node: u32, set: impl FnOnce(Params) -> Params) -> Result<Outputs> {
    let s = ctx.spec;
    let size = ctx.f64("feature_size_m");
    let (ox, oy) = (ctx.f64("offset_x_m"), ctx.f64("offset_y_m"));
    let height = gpu.field(ctx, "height_m")?;
    let mask = gpu.field_buffer(&height)?;
    let out = gpu.alloc(s.len())?;
    // Lattice position = f[0] + f[1] × column (and likewise for rows), with
    // the offsets folded in on the CPU in f64.
    let lattice = |axis: usize, n: u32| {
        (
            ((s.origin_m[axis] + if axis == 0 { ox } else { oy }) / size) as f32,
            (s.extent_m[axis] / (n - 1) as f64 / size) as f32,
        )
    };
    let (ax, bx) = lattice(0, s.width);
    let (ay, by) = lattice(1, s.height);
    let p = Params::grid(s)
        .u64(2, ctx.seed)
        .u(4, node)
        .u(8, height.driven())
        .f(0, ax)
        .f(1, bx)
        .f(2, ay)
        .f(3, by)
        .f(4, height.value)
        .f(5, ctx.f32("base_m"))
        .f(10, height.min)
        .f(11, height.max);
    gpu.dispatch_grid(&kernels::NOISE, &[&out, &mask], &set(p), s)?;
    Ok(Outputs::from([(
        "out".to_string(),
        gpu.value(PortType::Heightfield, s, out),
    )]))
}

/// [`fractal_params`] for the GPU kernel.
fn fractal_gpu(ctx: &EvalContext, p: Params) -> Params {
    let (basis, octaves, lacunarity, gain) = fractal(ctx);
    let basis = match basis {
        Basis::Perlin => 0,
        Basis::Simplex => 1,
        Basis::Value => 2,
    };
    p.u(5, basis)
        .u(6, octaves)
        .f(6, lacunarity as f32)
        .f(7, gain as f32)
}

fn schema(type_id: &str, label: &str, description: &str, mut params: Vec<ParamDef>) -> NodeSchema {
    let mut all = common_params();
    // Node-specific params go after feature size, before the generic ones.
    let tail = all.split_off(1);
    all.append(&mut params);
    all.extend(tail);
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: "Noise".into(),
        description: description.into(),
        inputs: vec![],
        outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
        params: all,
        gpu: true,
    }
}

/// Single-layer Perlin noise: smooth rolling hills.
pub struct Perlin {
    schema: NodeSchema,
}

impl Default for Perlin {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.perlin",
                "Perlin",
                "Smooth, rolling single-layer Perlin noise.",
                vec![],
            ),
        }
    }
}

impl NodeKind for Perlin {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        noise_heightfield(ctx, |x, y| basis::perlin(x, y, seed))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 0, |p| p)
    }
}

/// Single-layer simplex noise: like Perlin with fewer grid artefacts.
pub struct Simplex {
    schema: NodeSchema,
}

impl Default for Simplex {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.simplex",
                "Simplex",
                "Single-layer simplex noise: smooth, with fewer grid-aligned artefacts than Perlin.",
                vec![],
            ),
        }
    }
}

impl NodeKind for Simplex {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        noise_heightfield(ctx, |x, y| basis::simplex(x, y, seed))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 1, |p| p)
    }
}

/// Fractal noise: several octaves of Perlin or simplex, for natural detail.
pub struct Fbm {
    schema: NodeSchema,
}

impl Default for Fbm {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.fbm",
                "fBm",
                "Fractal noise: layers of Perlin or simplex noise at finer and finer scales.",
                fractal_params(6, 0.5),
            ),
        }
    }
}

/// Basis, octaves, lacunarity and roughness.
fn fractal_params(octaves: i64, gain: f64) -> Vec<ParamDef> {
    vec![
        ParamDef::choice(
            "basis",
            "Basis",
            "perlin",
            &[("perlin", "Perlin"), ("simplex", "Simplex"), ("value", "Value")],
        ),
        ParamDef::int("octaves", "Octaves", octaves, 1, 16).describe("Number of detail layers."),
        ParamDef::float("lacunarity", "Lacunarity", 2.0, 1.1, 4.0)
            .describe("How much finer each layer is than the last."),
        ParamDef::float("gain", "Roughness", gain, 0.0, 1.0)
            .describe("How strong each layer is compared with the last (persistence)."),
    ]
}

/// Read [`fractal_params`].
fn fractal(ctx: &EvalContext) -> (Basis, u32, f64, f64) {
    (
        Basis::from_key(&ctx.choice("basis")),
        ctx.i64("octaves") as u32,
        ctx.f64("lacunarity"),
        ctx.f64("gain"),
    )
}

impl NodeKind for Fbm {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let (basis, octaves, lacunarity, gain) = fractal(ctx);
        noise_heightfield(ctx, |x, y| {
            basis::fbm(basis, x, y, seed, octaves, lacunarity, gain)
        })
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 3, |p| fractal_gpu(ctx, p))
    }
}

/// Single-layer value noise: soft, blobby hills.
pub struct ValueNoise {
    schema: NodeSchema,
}

impl Default for ValueNoise {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.value",
                "Value",
                "Single-layer value noise: random heights smoothly blended. Softer and blobbier than Perlin.",
                vec![],
            ),
        }
    }
}

impl NodeKind for ValueNoise {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        noise_heightfield(ctx, |x, y| basis::value(x, y, seed))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 2, |p| p)
    }
}

/// Ridged multifractal: sharp crests and branching ridgelines.
pub struct Ridged {
    schema: NodeSchema,
}

impl Default for Ridged {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.ridged",
                "Ridged",
                "Ridged fractal noise: sharp, branching ridgelines with detail gathered on the crests. \
                 A good start for mountain ranges.",
                fractal_params(8, 0.5),
            ),
        }
    }
}

impl NodeKind for Ridged {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let (basis, octaves, lacunarity, gain) = fractal(ctx);
        noise_heightfield(ctx, |x, y| {
            basis::ridged(basis, x, y, seed, octaves, lacunarity, gain) * 2.0 - 1.0
        })
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 4, |p| fractal_gpu(ctx, p))
    }
}

/// Billow noise: rounded, puffy hills with creased valleys.
pub struct Billow {
    schema: NodeSchema,
}

impl Default for Billow {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.billow",
                "Billow",
                "Billow fractal noise: rounded, puffy hills separated by sharp creases.",
                fractal_params(6, 0.5),
            ),
        }
    }
}

impl NodeKind for Billow {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let (basis, octaves, lacunarity, gain) = fractal(ctx);
        noise_heightfield(ctx, |x, y| {
            basis::billow(basis, x, y, seed, octaves, lacunarity, gain)
        })
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        noise_gpu(ctx, gpu, 5, |p| fractal_gpu(ctx, p))
    }
}

/// fBm whose sampling position is pushed around by more fBm: swirling,
/// folded shapes.
pub struct DomainWarp {
    schema: NodeSchema,
}

impl Default for DomainWarp {
    fn default() -> Self {
        let mut params = fractal_params(6, 0.5);
        params.push(
            ParamDef::metres("warp_m", "Warp", 2000.0, 0.0, 100_000.0)
                .describe("How far the noise is pushed around, in metres."),
        );
        Self {
            schema: schema(
                "noise.domain_warp",
                "Domain Warp",
                "Fractal noise pushed around by other noise: swirling, folded, flowing shapes.",
                params,
            ),
        }
    }
}

impl NodeKind for DomainWarp {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let (basis, octaves, lacunarity, gain) = fractal(ctx);
        let warp = ctx.f64("warp_m") / ctx.f64("feature_size_m");
        noise_heightfield(ctx, |x, y| {
            basis::warped_fbm(basis, x, y, seed, octaves, lacunarity, gain, warp)
        })
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let warp = (ctx.f64("warp_m") / ctx.f64("feature_size_m")) as f32;
        noise_gpu(ctx, gpu, 6, |p| fractal_gpu(ctx, p).f(8, warp))
    }
}

/// Cellular (Worley/Voronoi) noise.
pub struct Voronoi {
    schema: NodeSchema,
}

impl Default for Voronoi {
    fn default() -> Self {
        Self {
            schema: schema(
                "noise.voronoi",
                "Voronoi",
                "Cellular noise: a random pattern of cells. Feature size is the typical cell size.",
                vec![
                    ParamDef::choice(
                        "mode",
                        "Mode",
                        "f1",
                        &[
                            ("f1", "Distance (cones rising to the edges)"),
                            ("f1_inverted", "Peaks (cones at the centres)"),
                            ("f2_f1", "Edges (ridges along cell borders)"),
                            ("cells", "Cells (flat random heights)"),
                        ],
                    ),
                    ParamDef::float("jitter", "Jitter", 1.0, 0.0, 1.0)
                        .describe("0 = a regular grid of cells, 1 = fully random."),
                ],
            ),
        }
    }
}

impl NodeKind for Voronoi {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let jitter = ctx.f64("jitter");
        let mode = ctx.choice("mode");
        // Each mode mapped to about -1..1.
        let f: fn(basis::Cells) -> f64 = match mode.as_str() {
            "f1_inverted" => |c| 1.0 - 2.0 * c.f1.min(1.0),
            "f2_f1" => |c| ((c.f2 - c.f1) * 2.0).min(1.0) * -2.0 + 1.0,
            "cells" => |c| c.cell_value * 2.0 - 1.0,
            _ => |c| 2.0 * c.f1.min(1.0) - 1.0,
        };
        noise_heightfield(ctx, |x, y| f(basis::cellular(x, y, seed, jitter)))
    }
    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let mode = match ctx.choice("mode").as_str() {
            "f1_inverted" => 1,
            "f2_f1" => 2,
            "cells" => 3,
            _ => 0,
        };
        noise_gpu(ctx, gpu, 7, |p| p.u(7, mode).f(9, ctx.f32("jitter")))
    }
}
