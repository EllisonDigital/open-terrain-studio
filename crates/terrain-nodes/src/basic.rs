//! Basic nodes: Constant, Combine and Levels.

use std::sync::Arc;

use terrain_core::error::Result;
use terrain_core::{EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType, Value};

fn heightfield_out(grid: Grid) -> Result<Outputs> {
    Ok(Outputs::from([(
        "out".to_string(),
        Value::Heightfield(Arc::new(grid)),
    )]))
}

// ---- Constant -------------------------------------------------------------

/// A flat terrain at a fixed height.
pub struct Constant {
    schema: NodeSchema,
}

impl Default for Constant {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "primitive.constant".into(),
                type_version: 1,
                label: "Constant".into(),
                category: "Primitives".into(),
                description: "A flat terrain at a fixed height.".into(),
                inputs: vec![],
                outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
                params: vec![ParamDef::metres("height_m", "Height", 0.0, -10_000.0, 20_000.0)],
                gpu: false,
            },
        }
    }
}

impl NodeKind for Constant {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        heightfield_out(Grid::filled(ctx.spec, ctx.f32("height_m")))
    }
}

// ---- Combine --------------------------------------------------------------

/// Combine two terrains.
///
/// `out = lerp(A, op(A, B), ratio × mask)`, so Ratio fades the effect in and an
/// optional Mask limits where it applies. Multiply works on heights normalised
/// to the world height range, so the result stays in range.
pub struct Combine {
    schema: NodeSchema,
}

impl Default for Combine {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "combine.combine".into(),
                type_version: 1,
                label: "Combine".into(),
                category: "Combine".into(),
                description: "Combine two terrains: add, subtract, multiply, max, min or blend. \
                              Ratio fades the effect; the optional mask limits where it applies."
                    .into(),
                inputs: vec![
                    PortDef::new("a", "A", PortType::Heightfield),
                    PortDef::new("b", "B", PortType::Heightfield),
                    PortDef::new("mask", "Mask", PortType::Mask).optional(),
                ],
                outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
                params: vec![
                    ParamDef::choice(
                        "mode",
                        "Mode",
                        "add",
                        &[
                            ("add", "Add"),
                            ("subtract", "Subtract"),
                            ("multiply", "Multiply"),
                            ("max", "Max"),
                            ("min", "Min"),
                            ("blend", "Blend"),
                        ],
                    ),
                    ParamDef::float("ratio", "Ratio", 1.0, 0.0, 1.0)
                        .describe("0 = only A, 1 = full effect. For Blend, 0.5 is an even mix."),
                ],
                gpu: false,
            },
        }
    }
}

impl NodeKind for Combine {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let a = ctx.input_grid("a")?;
        let b = ctx.input_grid("b")?;
        let mask = ctx.input("mask").map(|m| m.grid().clone());
        let ratio = ctx.f32("ratio");
        let mode = ctx.choice("mode");
        let world = ctx.world;

        let op: fn(f32, f32, &terrain_core::World) -> f32 = match mode.as_str() {
            "subtract" => |a, b, _| a - b,
            "multiply" => |a, b, w| w.denormalise(w.normalise(a) * w.normalise(b)),
            "max" => |a, b, _| a.max(b),
            "min" => |a, b, _| a.min(b),
            "blend" => |_, b, _| b,
            _ => |a, b, _| a + b,
        };

        let mut out = (**a).clone();
        for (idx, v) in out.data.iter_mut().enumerate() {
            let av = a.data[idx];
            let w = ratio * mask.as_ref().map_or(1.0, |m| m.data[idx].clamp(0.0, 1.0));
            let target = op(av, b.data[idx], world);
            *v = av + (target - av) * w;
        }
        heightfield_out(out)
    }
}

// ---- Levels ---------------------------------------------------------------

/// Remap heights from an input range to an output range, with gamma.
pub struct Levels {
    schema: NodeSchema,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "adjust.levels".into(),
                type_version: 1,
                label: "Levels".into(),
                category: "Adjust".into(),
                description: "Remap heights from an input range to an output range. \
                              Gamma below 1 raises the midtones; above 1 lowers them."
                    .into(),
                inputs: vec![PortDef::new("in", "In", PortType::Heightfield)],
                outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
                params: vec![
                    ParamDef::bool("auto_input", "Auto input range", true)
                        .describe("Use the input's own lowest and highest points as the input range."),
                    ParamDef::metres("in_low_m", "Input low", 0.0, -10_000.0, 20_000.0),
                    ParamDef::metres("in_high_m", "Input high", 2000.0, -10_000.0, 20_000.0),
                    ParamDef::bool("world_output", "Output = world height range", true).describe(
                        "Stretch the result over the project's full height range (World settings).",
                    ),
                    ParamDef::metres("out_low_m", "Output low", 0.0, -10_000.0, 20_000.0),
                    ParamDef::metres("out_high_m", "Output high", 2000.0, -10_000.0, 20_000.0),
                    ParamDef::float("gamma", "Gamma", 1.0, 0.1, 10.0),
                    ParamDef::bool("clamp", "Clamp", true).describe("Keep results inside the output range."),
                ],
                gpu: false,
            },
        }
    }
}

impl NodeKind for Levels {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        // Note: auto range uses the min/max of this grid. Tiled builds (v0.8)
        // will need a global pre-pass so every tile uses the same range.
        let (in_lo, in_hi) = if ctx.bool("auto_input") {
            input.min_max()
        } else {
            (ctx.f32("in_low_m"), ctx.f32("in_high_m"))
        };
        let (out_lo, out_hi) = if ctx.bool("world_output") {
            (ctx.world.height_range_m[0], ctx.world.height_range_m[1])
        } else {
            (ctx.f32("out_low_m"), ctx.f32("out_high_m"))
        };
        let gamma = ctx.f32("gamma");
        let clamp = ctx.bool("clamp");
        let span = if (in_hi - in_lo).abs() < 1e-6 {
            1.0
        } else {
            in_hi - in_lo
        };

        let out = input.map(|h| {
            let mut t = (h - in_lo) / span;
            if clamp {
                t = t.clamp(0.0, 1.0);
            }
            // libm gives identical results on every platform (std's powf may not).
            let t = if gamma == 1.0 {
                t
            } else if t >= 0.0 {
                libm::powf(t, gamma)
            } else {
                -libm::powf(-t, gamma)
            };
            out_lo + t * (out_hi - out_lo)
        });
        heightfield_out(out)
    }
}
