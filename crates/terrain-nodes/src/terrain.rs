//! Landform generators: Mountain, Ridge, Canyon, Crater, Plateau and Dunes.
//!
//! First versions (v0.2): each is a hand-built profile shaped by noise, meant
//! to be combined and then eroded (v0.3). All sizes are metres and all noise
//! is sampled at world positions, so they look the same at any resolution.

use terrain_core::error::Result;
use terrain_core::ops::smoothstep64;
use terrain_core::seed::derive;
use terrain_core::{EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType};

use crate::common::{
    angle_param, direction, height_params, heightfield_out, position, position_params, seed_param,
};
use crate::noise::basis::{self, Basis};

fn terrain_schema(
    type_id: &str,
    label: &str,
    description: &str,
    inputs: Vec<PortDef>,
    params: Vec<ParamDef>,
) -> NodeSchema {
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: "Terrain".into(),
        description: description.into(),
        inputs,
        outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
        params,
        gpu: false,
    }
}

fn roughness_param(default: f64) -> ParamDef {
    ParamDef::float("roughness", "Roughness", default, 0.0, 1.0)
        .describe("Amount of irregular, noisy detail.")
}

/// fBm (-1..1) at a world position with a feature size in metres.
#[inline]
fn fbm_m(x: f64, y: f64, size: f64, seed: u64, octaves: u32) -> f64 {
    basis::fbm(Basis::Perlin, x / size, y / size, seed, octaves, 2.0, 0.5)
}

/// Optional carving input: the terrain a feature is cut into or placed on.
fn optional_base(ctx: &EvalContext, idx: usize, default: f64) -> f64 {
    match ctx.input("in") {
        Some(v) => v.grid().data[idx] as f64,
        None => default,
    }
}

// ---- Mountain ---------------------------------------------------------------

/// A single mountain with ridges running down from the summit.
pub struct Mountain {
    schema: NodeSchema,
}

impl Default for Mountain {
    fn default() -> Self {
        let mut params = vec![
            ParamDef::metres("radius_m", "Radius", 3500.0, 10.0, 1_000_000.0)
                .describe("Radius of the mountain's footprint, in metres."),
        ];
        params.extend(height_params(1800.0, 0.0));
        params.extend([
            ParamDef::choice(
                "style",
                "Style",
                "alpine",
                &[
                    ("alpine", "Alpine (sharp ridges)"),
                    ("rounded", "Rounded (old, worn hills)"),
                ],
            ),
            ParamDef::float("ridges", "Ridges", 0.6, 0.0, 1.0)
                .describe("How strongly ridges and gullies cut into the flanks."),
            roughness_param(0.5),
        ]);
        params.extend(position_params());
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.mountain",
                "Mountain",
                "A single mountain with ridges and gullies running down from the summit.",
                vec![],
                params,
            ),
        }
    }
}

impl NodeKind for Mountain {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let radius = ctx.f64("radius_m");
        let height = ctx.field("height_m");
        let base = ctx.f64("base_m");
        let ridges = ctx.f64("ridges");
        let rough = ctx.f64("roughness");
        let alpine = ctx.choice("style") == "alpine";
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            // An irregular footprint: warp the distance from the centre.
            let wx = fbm_m(x, y, radius * 1.2, derive(seed, 1), 3);
            let wy = fbm_m(x + 1234.5, y, radius * 1.2, derive(seed, 2), 3);
            let (px, py) = (x - cx + wx * radius * 0.35, y - cy + wy * radius * 0.35);
            let r = (px * px + py * py).sqrt() / radius;
            if r >= 1.0 {
                return base as f32;
            }
            // Concave flanks, rounded or pointed summit.
            let body = if alpine {
                (1.0 - r) * (1.0 - r) * (1.0 + r * 0.6)
            } else {
                let t = 1.0 - r * r;
                t * t
            };
            // Ridges run down from the summit: sample ridged noise on a circle
            // that grows slowly with distance, so it changes quickly around the
            // mountain and slowly along each radius (seam-free polar coordinates).
            let len = (px * px + py * py).sqrt().max(1e-9);
            let k = 1.3 + r * 1.1;
            let (qx, qy) = (px / len * k, py / len * k);
            let radial = basis::ridged(
                Basis::Perlin,
                qx + 31.7,
                qy - 12.9,
                derive(seed, 3),
                if alpine { 7 } else { 4 },
                2.0,
                if alpine { 0.5 } else { 0.4 },
            );
            let random = basis::ridged(
                Basis::Perlin,
                x / (radius * 0.5),
                y / (radius * 0.5),
                derive(seed, 5),
                if alpine { 7 } else { 4 },
                2.0,
                0.5,
            );
            // Near the summit the polar pattern pinches; blend towards plain noise there.
            let rid = random + (radial - random) * smoothstep64(0.0, 0.35, r) * 0.75;
            let carve = ridges * (1.0 - r * 0.3);
            let shaped = body * (1.0 - carve + carve * (rid * 1.35).min(1.0));
            let detail = fbm_m(x, y, radius * 0.12, derive(seed, 4), 6) * rough * 0.08 * body.sqrt();
            (base + height.at(idx) as f64 * (shaped + detail).max(0.0)) as f32
        }))
    }
}

// ---- Ridge ------------------------------------------------------------------

/// A long mountain ridge.
pub struct Ridge {
    schema: NodeSchema,
}

impl Default for Ridge {
    fn default() -> Self {
        let mut params = vec![
            angle_param(
                "angle_deg",
                "Direction",
                30.0,
                "Direction the ridge runs in. 0° = +X (right).",
            ),
            ParamDef::metres("length_m", "Length", 6000.0, 10.0, 1_000_000.0)
                .describe("Length of the ridge crest, in metres."),
            ParamDef::metres("width_m", "Width", 1500.0, 10.0, 1_000_000.0)
                .describe("Distance from the crest to the foot of the slopes, in metres."),
        ];
        params.extend(height_params(1200.0, 0.0));
        params.extend([
            ParamDef::metres("meander_m", "Meander", 500.0, 0.0, 100_000.0)
                .describe("How far the crest line wanders from a straight line, in metres."),
            ParamDef::float("ridges", "Spurs", 0.5, 0.0, 1.0)
                .describe("How strongly side spurs and gullies cut into the slopes."),
            roughness_param(0.5),
        ]);
        params.extend(position_params());
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.ridge",
                "Ridge",
                "A long mountain ridge with a wandering crest and spurs down its sides.",
                vec![],
                params,
            ),
        }
    }
}

impl NodeKind for Ridge {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let (dx, dy) = direction(ctx.f64("angle_deg"));
        let len = ctx.f64("length_m");
        let width = ctx.f64("width_m");
        let height = ctx.field("height_m");
        let base = ctx.f64("base_m");
        let meander = ctx.f64("meander_m");
        let spurs = ctx.f64("ridges");
        let rough = ctx.f64("roughness");
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let (px, py) = (x - cx, y - cy);
            // u along the ridge, v across it.
            let u = px * dx + py * dy;
            let mut v = -px * dy + py * dx;
            v += meander * fbm_m(u, 0.0, len * 0.4, derive(seed, 1), 3);
            let over = (u.abs() - len * 0.5).max(0.0);
            let d = (v * v + over * over).sqrt() / width;
            if d >= 1.0 {
                return base as f32;
            }
            let body = libm::pow(1.0 - d, 1.6);
            // The crest rises and falls along its length.
            let crest = 0.8 + 0.2 * fbm_m(u, 0.0, len * 0.25, derive(seed, 2), 3);
            // Stretched across the ridge, so spurs and gullies run down its sides.
            let rid = basis::ridged(
                Basis::Perlin,
                u / (width * 0.45),
                v / (width * 1.4),
                derive(seed, 3),
                7,
                2.0,
                0.5,
            );
            let carve = spurs * d.min(1.0).sqrt();
            let shaped = body * crest * (1.0 - carve + carve * (rid * 1.35).min(1.0));
            let detail = fbm_m(x, y, width * 0.15, derive(seed, 4), 5) * rough * 0.08 * body.sqrt();
            (base + height.at(idx) as f64 * (shaped + detail).max(0.0)) as f32
        }))
    }
}

// ---- Canyon -----------------------------------------------------------------

/// A winding canyon cut across the terrain.
pub struct Canyon {
    schema: NodeSchema,
}

impl Default for Canyon {
    fn default() -> Self {
        let mut params = vec![
            angle_param(
                "angle_deg",
                "Direction",
                0.0,
                "Direction the canyon runs in. 0° = +X (right).",
            ),
            ParamDef::metres("depth_m", "Depth", 450.0, 0.0, 10_000.0)
                .describe("Depth from the rim to the floor, in metres.")
                .drivable(),
            ParamDef::metres("width_m", "Width", 1800.0, 1.0, 100_000.0)
                .describe("Width from rim to rim, in metres."),
            ParamDef::metres("floor_width_m", "Floor width", 300.0, 0.0, 100_000.0)
                .describe("Width of the flat floor, in metres."),
            ParamDef::int("steps", "Steps", 3, 0, 12)
                .describe("Number of ledges in the walls (hard rock layers). 0 = smooth walls."),
            ParamDef::metres("meander_m", "Meander", 900.0, 0.0, 100_000.0)
                .describe("How far the canyon winds from side to side, in metres."),
            ParamDef::metres("meander_size_m", "Meander length", 3000.0, 10.0, 1_000_000.0)
                .describe("Distance between bends, in metres."),
            roughness_param(0.5),
            ParamDef::metres("surface_m", "Surface height", 800.0, -10_000.0, 20_000.0)
                .describe("Height of the land the canyon is cut into, when no input is connected."),
        ];
        params.extend(position_params());
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.canyon",
                "Canyon",
                "A winding canyon with a flat floor and stepped walls, cut into the input terrain \
                 (or into flat land at Surface height).",
                vec![PortDef::new("in", "In", PortType::Heightfield).optional()],
                params,
            ),
        }
    }
}

impl NodeKind for Canyon {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let (dx, dy) = direction(ctx.f64("angle_deg"));
        let depth = ctx.field("depth_m");
        let half = ctx.f64("width_m") * 0.5;
        let floor = (ctx.f64("floor_width_m") * 0.5).min(half * 0.95);
        let steps = ctx.i64("steps") as f64;
        let meander = ctx.f64("meander_m");
        let msize = ctx.f64("meander_size_m");
        let rough = ctx.f64("roughness");
        let surface = ctx.f64("surface_m");
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let ground = optional_base(ctx, idx, surface);
            let (px, py) = (x - cx, y - cy);
            let u = px * dx + py * dy;
            let v = -px * dy + py * dx;
            // Winding centre line, plus ragged walls.
            let off = meander * fbm_m(u, 0.0, msize, derive(seed, 1), 3);
            let ragged = rough
                * (half - floor)
                * 0.35
                * fbm_m(x, y, (half - floor).max(1.0) * 0.8, derive(seed, 2), 4);
            let d = (v - off).abs() + ragged;
            if d >= half {
                return ground as f32;
            }
            // t: 0 at the floor edge, 1 at the rim.
            let t = ((d - floor) / (half - floor)).clamp(0.0, 1.0);
            let mut wall = smoothstep64(0.0, 1.0, t);
            if steps > 0.0 {
                // Ledges: flat benches with steep risers between them.
                let s = wall * steps;
                let k = s.floor();
                let f = s - k;
                let stepped = (k + smoothstep64(0.55, 1.0, f)) / steps;
                wall = stepped * 0.8 + wall * 0.2;
            }
            (ground - depth.at(idx) as f64 * (1.0 - wall)) as f32
        }))
    }
}

// ---- Crater -----------------------------------------------------------------

/// An impact crater with a raised rim.
pub struct Crater {
    schema: NodeSchema,
}

impl Default for Crater {
    fn default() -> Self {
        let mut params = vec![
            ParamDef::metres("radius_m", "Radius", 1200.0, 1.0, 1_000_000.0)
                .describe("Radius of the rim, in metres."),
            ParamDef::metres("depth_m", "Depth", 300.0, 0.0, 10_000.0)
                .describe("Depth of the bowl below the surrounding ground, in metres.")
                .drivable(),
            ParamDef::metres("rim_height_m", "Rim height", 120.0, 0.0, 10_000.0)
                .describe("Height of the rim above the surrounding ground, in metres."),
            ParamDef::float("rim_width", "Rim width", 0.6, 0.05, 4.0)
                .describe("How far the raised rim and ejecta spread outside, as a fraction of the radius."),
            ParamDef::float("floor", "Flat floor", 0.3, 0.0, 0.9)
                .describe("Size of the flat floor, as a fraction of the radius."),
            roughness_param(0.3),
            ParamDef::metres("base_m", "Base", 0.0, -10_000.0, 20_000.0)
                .describe("Height of the surrounding ground, when no input is connected."),
        ];
        params.extend(position_params());
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.crater",
                "Crater",
                "An impact crater: a bowl with a raised rim and a flat floor, placed on the input \
                 terrain (or on flat ground).",
                vec![PortDef::new("in", "In", PortType::Heightfield).optional()],
                params,
            ),
        }
    }
}

impl NodeKind for Crater {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let radius = ctx.f64("radius_m");
        let depth = ctx.field("depth_m");
        let rim = ctx.f64("rim_height_m");
        let rim_w = ctx.f64("rim_width");
        let floor = ctx.f64("floor");
        let rough = ctx.f64("roughness");
        let base = ctx.f64("base_m");
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let ground = optional_base(ctx, idx, base);
            let (px, py) = (x - cx, y - cy);
            let wobble = 1.0 + rough * 0.15 * fbm_m(x, y, radius * 0.5, derive(seed, 1), 3);
            let r = (px * px + py * py).sqrt() / (radius * wobble);
            let depth = depth.at(idx) as f64;
            let h = if r < 1.0 {
                // Bowl: flat floor, then rising to the rim crest at r = 1.
                let t = ((r - floor) / (1.0 - floor)).clamp(0.0, 1.0);
                -depth + (depth + rim) * t * t * (1.2 - 0.2 * t)
            } else {
                // Outer rim slope and ejecta blanket.
                let t = (r - 1.0) / rim_w;
                rim / (1.0 + 6.0 * t * t) * (1.0 - smoothstep64(0.6, 1.0, t / 3.0))
            };
            let detail = rough
                * 0.1
                * (depth + rim)
                * fbm_m(x, y, radius * 0.15, derive(seed, 2), 4)
                * (1.0 - smoothstep64(1.0, 1.0 + rim_w * 2.0, r));
            (ground + h + detail) as f32
        }))
    }
}

// ---- Plateau ----------------------------------------------------------------

/// A mesa: flat top, cliffs, and a talus slope at the foot.
pub struct Plateau {
    schema: NodeSchema,
}

impl Default for Plateau {
    fn default() -> Self {
        let mut params = vec![
            ParamDef::metres("radius_m", "Radius", 2200.0, 1.0, 1_000_000.0)
                .describe("Radius of the flat top, in metres."),
        ];
        params.extend(height_params(500.0, 0.0));
        params.extend([
            ParamDef::metres("cliff_m", "Cliff width", 150.0, 0.0, 100_000.0)
                .describe("Horizontal width of the steep cliff band, in metres."),
            ParamDef::metres("talus_m", "Talus width", 700.0, 0.0, 100_000.0)
                .describe("Width of the gentler rubble slope at the foot of the cliffs, in metres."),
            ParamDef::float("talus_height", "Talus height", 0.35, 0.0, 1.0)
                .describe("Height where the cliff meets the talus slope, as a fraction of Height."),
            ParamDef::metres("edge_m", "Edge roughness", 600.0, 0.0, 100_000.0)
                .describe("How far the cliff edge wanders in and out, in metres."),
            roughness_param(0.3),
        ]);
        params.extend(position_params());
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.plateau",
                "Plateau",
                "A mesa: a flat top ringed by cliffs, with a rubble slope at their foot.",
                vec![],
                params,
            ),
        }
    }
}

impl NodeKind for Plateau {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let radius = ctx.f64("radius_m");
        let height = ctx.field("height_m");
        let base = ctx.f64("base_m");
        let cliff = ctx.f64("cliff_m").max(1e-3);
        let talus = ctx.f64("talus_m").max(1e-3);
        let talus_h = ctx.f64("talus_height");
        let edge = ctx.f64("edge_m");
        let rough = ctx.f64("roughness");
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let (px, py) = (x - cx, y - cy);
            let wander = edge * fbm_m(x, y, (edge * 2.5).max(radius * 0.3), derive(seed, 1), 4);
            // Distance outside the top's edge (negative = on top).
            let d = (px * px + py * py).sqrt() - radius + wander;
            let p = if d <= 0.0 {
                1.0 + rough * 0.03 * fbm_m(x, y, radius * 0.3, derive(seed, 2), 4)
            } else if d < cliff {
                let t = d / cliff;
                1.0 - (1.0 - talus_h) * smoothstep64(0.0, 1.0, t)
            } else {
                let t = ((d - cliff) / talus).min(1.0);
                talus_h * (1.0 - t) * (1.0 - t)
            };
            (base + height.at(idx) as f64 * p) as f32
        }))
    }
}

// ---- Dunes ------------------------------------------------------------------

/// A field of wind-built sand dunes.
pub struct Dunes {
    schema: NodeSchema,
}

impl Default for Dunes {
    fn default() -> Self {
        let mut params = vec![
            angle_param(
                "angle_deg",
                "Wind direction",
                0.0,
                "Direction the wind blows towards. Steep slip faces face this way. 0° = +X (right).",
            ),
            ParamDef::metres("wavelength_m", "Spacing", 450.0, 1.0, 100_000.0)
                .describe("Distance between dune crests, in metres."),
        ];
        params.extend(height_params(35.0, 0.0));
        params.extend([
            ParamDef::float("asymmetry", "Asymmetry", 0.75, 0.5, 0.95)
                .describe("Share of each dune taken by the gentle windward slope. 0.5 = symmetric."),
            ParamDef::metres("sinuosity_m", "Sinuosity", 220.0, 0.0, 100_000.0)
                .describe("How far the crests snake back and forth, in metres."),
            ParamDef::float("variation", "Height variation", 0.5, 0.0, 1.0)
                .describe("How much dune height varies across the field."),
        ]);
        params.push(seed_param());
        Self {
            schema: terrain_schema(
                "terrain.dunes",
                "Dunes",
                "A field of transverse sand dunes: gentle windward slopes, steep slip faces and \
                 sinuous crests.",
                vec![],
                params,
            ),
        }
    }
}

impl NodeKind for Dunes {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (dx, dy) = direction(ctx.f64("angle_deg"));
        let lambda = ctx.f64("wavelength_m");
        let height = ctx.field("height_m");
        let base = ctx.f64("base_m");
        let a = ctx.f64("asymmetry");
        let sinuosity = ctx.f64("sinuosity_m");
        let variation = ctx.f64("variation");
        let seed = ctx.seed;
        heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
            let u = x * dx + y * dy;
            let v = -x * dy + y * dx;
            // Crests snake sideways; the phase also drifts so crests split and merge.
            let snake = sinuosity * fbm_m(v, u * 0.3, lambda * 1.6, derive(seed, 1), 3);
            let drift = 0.6 * lambda * fbm_m(x, y, lambda * 3.0, derive(seed, 2), 3);
            let phase = (u + snake + drift) / lambda;
            let f = phase - phase.floor();
            let p = if f < a {
                // Windward: a long, gently convex rise.
                let t = f / a;
                t * t * (3.0 - 2.0 * t) * 0.3 + t * 0.7
            } else {
                // Slip face: steep just below the crest, flattening at the foot.
                let t = (f - a) / (1.0 - a);
                (1.0 - t) * (1.0 - t)
            };
            let amp = 1.0 - variation * (0.5 + 0.5 * fbm_m(x, y, lambda * 4.0, derive(seed, 3), 3));
            (base + height.at(idx) as f64 * p * amp) as f32
        }))
    }
}
