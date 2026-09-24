//! Noise nodes: Perlin, Simplex and fBm.
//!
//! All sizes are in metres and noise is sampled at world positions, so a
//! feature sits in the same place at any resolution (ARCHITECTURE.md §4).

pub mod basis;

use std::sync::Arc;

use terrain_core::error::Result;
use terrain_core::{EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType, Value};

use basis::Basis;

/// Parameters shared by every noise node.
fn common_params() -> Vec<ParamDef> {
    vec![
        ParamDef::metres("feature_size_m", "Feature size", 3000.0, 10.0, 100_000.0)
            .describe("Size of the largest features, in metres."),
        ParamDef::metres("height_m", "Height", 1000.0, 0.0, 20_000.0)
            .describe("Difference between the lowest and highest points, in metres."),
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
    let base = ctx.f64("base_m");
    let (ox, oy) = (ctx.f64("offset_x_m"), ctx.f64("offset_y_m"));
    let grid = Grid::from_fn(ctx.spec, |x, y| {
        let n = f((x + ox) / size, (y + oy) / size);
        (base + (n * 0.5 + 0.5) * height) as f32
    });
    Ok(Outputs::from([(
        "out".to_string(),
        Value::Heightfield(Arc::new(grid)),
    )]))
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
        gpu: false,
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
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        noise_heightfield(ctx, |x, y| basis::perlin(x, y, seed))
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
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        noise_heightfield(ctx, |x, y| basis::simplex(x, y, seed))
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
                vec![
                    ParamDef::choice(
                        "basis",
                        "Basis",
                        "perlin",
                        &[("perlin", "Perlin"), ("simplex", "Simplex")],
                    ),
                    ParamDef::int("octaves", "Octaves", 6, 1, 16).describe("Number of detail layers."),
                    ParamDef::float("lacunarity", "Lacunarity", 2.0, 1.1, 4.0)
                        .describe("How much finer each layer is than the last."),
                    ParamDef::float("gain", "Roughness", 0.5, 0.0, 1.0)
                        .describe("How strong each layer is compared with the last (persistence)."),
                ],
            ),
        }
    }
}

impl NodeKind for Fbm {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let seed = ctx.seed;
        let basis = if ctx.choice("basis") == "simplex" {
            Basis::Simplex
        } else {
            Basis::Perlin
        };
        let octaves = ctx.i64("octaves") as u32;
        let lacunarity = ctx.f64("lacunarity");
        let gain = ctx.f64("gain");
        noise_heightfield(ctx, |x, y| {
            basis::fbm(basis, x, y, seed, octaves, lacunarity, gain)
        })
    }
}
