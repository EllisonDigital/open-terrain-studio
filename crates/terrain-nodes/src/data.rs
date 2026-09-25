//! Data nodes: masks measured from the terrain (Height mask, Slope,
//! Curvature, Aspect) or from other masks (Select range, Distance).
//! Every output is a Mask (0..1).

use terrain_core::error::Result;
use terrain_core::ops::{gaussian_blur, gradient, slope_degrees, smoothstep, soft_range};
use terrain_core::{EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType};

use crate::common::{angle_param, direction, mask_out};

fn data_schema(
    type_id: &str,
    label: &str,
    description: &str,
    input: PortType,
    mut params: Vec<ParamDef>,
) -> NodeSchema {
    params.push(ParamDef::bool("invert", "Invert", false).describe("Swap black and white."));
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: "Data".into(),
        description: description.into(),
        inputs: vec![PortDef::new("in", "In", input)],
        outputs: vec![PortDef::new("out", "Out", PortType::Mask)],
        params,
        gpu: false,
    }
}

/// Apply the shared "invert" parameter and output the mask.
fn finish(ctx: &EvalContext, mut grid: Grid) -> Result<Outputs> {
    if ctx.bool("invert") {
        for v in &mut grid.data {
            *v = 1.0 - v.clamp(0.0, 1.0);
        }
    }
    mask_out(grid)
}

// ---- Height mask ------------------------------------------------------------

/// Select a band of heights.
pub struct HeightMask {
    schema: NodeSchema,
}

impl Default for HeightMask {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.height_mask",
                "Height Mask",
                "White where the terrain is between Low and High, fading to black over Falloff.",
                PortType::Heightfield,
                vec![
                    ParamDef::metres("low_m", "Low", 1000.0, -10_000.0, 20_000.0),
                    ParamDef::metres("high_m", "High", 2000.0, -10_000.0, 20_000.0),
                    ParamDef::metres("falloff_m", "Falloff", 100.0, 0.0, 10_000.0)
                        .describe("Height over which the mask fades out, in metres."),
                ],
            ),
        }
    }
}

impl NodeKind for HeightMask {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let (lo, hi, f) = (ctx.f32("low_m"), ctx.f32("high_m"), ctx.f32("falloff_m"));
        finish(ctx, input.map(|h| soft_range(h, lo, hi, f)))
    }
}

// ---- Slope ------------------------------------------------------------------

/// Select a range of slope angles.
pub struct Slope {
    schema: NodeSchema,
}

impl Default for Slope {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.slope",
                "Slope",
                "White where the ground is as steep as Min to Max degrees (0° = flat, 90° = vertical).",
                PortType::Heightfield,
                vec![
                    ParamDef::float("min_deg", "Min angle", 30.0, 0.0, 90.0).unit("°"),
                    ParamDef::float("max_deg", "Max angle", 90.0, 0.0, 90.0).unit("°"),
                    ParamDef::float("falloff_deg", "Falloff", 5.0, 0.0, 45.0)
                        .unit("°")
                        .describe("Angle over which the mask fades out."),
                ],
            ),
        }
    }
}

impl NodeKind for Slope {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let (lo, hi, f) = (ctx.f32("min_deg"), ctx.f32("max_deg"), ctx.f32("falloff_deg"));
        let slope = slope_degrees(input);
        finish(ctx, slope.map(|s| soft_range(s, lo, hi, f)))
    }
}

// ---- Curvature --------------------------------------------------------------

/// Convex (ridges, peaks) or concave (valleys, hollows) areas.
pub struct Curvature {
    schema: NodeSchema,
}

impl Default for Curvature {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.curvature",
                "Curvature",
                "Finds ridges and peaks (convex) or valleys and hollows (concave), by comparing each \
                 point with the average height around it.",
                PortType::Heightfield,
                vec![
                    ParamDef::choice(
                        "mode",
                        "Show",
                        "convex",
                        &[
                            ("convex", "Convex (ridges)"),
                            ("concave", "Concave (valleys)"),
                            ("both", "Both (grey = flat)"),
                        ],
                    ),
                    ParamDef::metres("radius_m", "Radius", 150.0, 1.0, 100_000.0)
                        .describe("Size of the features to find, in metres."),
                    ParamDef::metres("range_m", "Sensitivity", 15.0, 0.01, 10_000.0).describe(
                        "Height above (or below) the surroundings that gives full white, in metres.",
                    ),
                ],
            ),
        }
    }
}

impl NodeKind for Curvature {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let range = ctx.f32("range_m");
        let blurred = gaussian_blur(input, ctx.f64("radius_m") * 0.5);
        let mode = ctx.choice("mode");
        finish(
            ctx,
            input.map_indexed(|idx, h| {
                let c = (h - blurred.data[idx]) / range;
                match mode.as_str() {
                    "concave" => -c,
                    "both" => 0.5 + 0.5 * c,
                    _ => c,
                }
            }),
        )
    }
}

// ---- Aspect -----------------------------------------------------------------

/// Slopes facing a direction.
pub struct Aspect {
    schema: NodeSchema,
}

impl Default for Aspect {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.aspect",
                "Aspect",
                "White on slopes facing the chosen direction (e.g. sunny or shaded sides), black on \
                 slopes facing away. Flat ground is black.",
                PortType::Heightfield,
                vec![
                    angle_param(
                        "direction_deg",
                        "Facing",
                        270.0,
                        "Direction the slopes face. 0° = +X (right), 90° = +Y (down), 270° = up in the 2D view.",
                    ),
                    ParamDef::float("sharpness", "Sharpness", 2.0, 0.1, 16.0)
                        .describe("Higher = only slopes facing almost exactly that way."),
                    ParamDef::float("min_slope_deg", "Min slope", 5.0, 0.0, 45.0)
                        .unit("°")
                        .describe("Ground flatter than this fades to black."),
                ],
            ),
        }
    }
}

impl NodeKind for Aspect {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let (fx, fy) = direction(ctx.f64("direction_deg"));
        let (fx, fy) = (fx as f32, fy as f32);
        let sharp = ctx.f32("sharpness");
        let min_slope = ctx.f32("min_slope_deg");
        let (gx, gy) = gradient(input);
        let data = gx
            .data
            .iter()
            .zip(&gy.data)
            .map(|(&x, &y)| {
                let len = (x * x + y * y).sqrt();
                if len <= 0.0 {
                    return 0.0;
                }
                // A slope faces downhill: along -gradient.
                let facing = (-x * fx - y * fy) / len;
                let slope = libm::atanf(len).to_degrees();
                libm::powf(0.5 + 0.5 * facing, sharp) * smoothstep(0.0, min_slope, slope)
            })
            .collect();
        finish(
            ctx,
            Grid {
                spec: input.spec,
                data,
            },
        )
    }
}

// ---- Select range -----------------------------------------------------------

/// Select a band of mask values.
pub struct SelectRange {
    schema: NodeSchema,
}

impl Default for SelectRange {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.select_range",
                "Select Range",
                "White where the input mask is between Low and High, fading to black over Falloff.",
                PortType::Mask,
                vec![
                    ParamDef::float("low", "Low", 0.4, 0.0, 1.0),
                    ParamDef::float("high", "High", 0.6, 0.0, 1.0),
                    ParamDef::float("falloff", "Falloff", 0.05, 0.0, 1.0),
                ],
            ),
        }
    }
}

impl NodeKind for SelectRange {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let (lo, hi, f) = (ctx.f32("low"), ctx.f32("high"), ctx.f32("falloff"));
        finish(ctx, input.map(|v| soft_range(v, lo, hi, f)))
    }
}

// ---- Distance ---------------------------------------------------------------

/// Distance from (or inside) the white areas of a mask.
pub struct Distance {
    schema: NodeSchema,
}

impl Default for Distance {
    fn default() -> Self {
        Self {
            schema: data_schema(
                "data.distance",
                "Distance",
                "A glow around the white areas of a mask: white at the shapes, fading to black at \
                 Distance away. Inside mode measures inwards from the edges instead.",
                PortType::Mask,
                vec![
                    ParamDef::choice(
                        "mode",
                        "Measure",
                        "outside",
                        &[("outside", "Outside the shapes"), ("inside", "Inside the shapes")],
                    ),
                    ParamDef::metres("distance_m", "Distance", 500.0, 0.01, 1_000_000.0)
                        .describe("Distance at which the fade ends, in metres."),
                    ParamDef::float("threshold", "Threshold", 0.5, 0.0, 1.0)
                        .describe("Mask values above this count as inside the shapes."),
                    ParamDef::choice(
                        "profile",
                        "Profile",
                        "linear",
                        &[("linear", "Linear"), ("smooth", "Smooth"), ("round", "Rounded")],
                    ),
                ],
            ),
        }
    }
}

impl NodeKind for Distance {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let threshold = ctx.f32("threshold");
        let dist = ctx.f32("distance_m");
        let inside_mode = ctx.choice("mode") == "inside";
        let profile = ctx.choice("profile");
        // Outside: distance to the nearest inside cell; inside: to the nearest outside cell.
        let targets: Vec<bool> = input
            .data
            .iter()
            .map(|&v| (v > threshold) != inside_mode)
            .collect();
        let d = terrain_core::ops::distance_to(input.spec, &targets);
        let shape = |t: f32| match profile.as_str() {
            "smooth" => smoothstep(0.0, 1.0, t),
            "round" => (1.0 - (1.0 - t) * (1.0 - t)).max(0.0).sqrt(),
            _ => t,
        };
        finish(
            ctx,
            d.map(|m| {
                let t = (m / dist).clamp(0.0, 1.0);
                if inside_mode { shape(t) } else { shape(1.0 - t) }
            }),
        )
    }
}
