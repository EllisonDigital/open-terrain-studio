//! Vegetation (ARCHITECTURE.md §8, `docs/vegetation.md`): Trees, Shrubs and
//! Grass populations, Debris / Rocks, and packing masks into RGBA.
//!
//! A population turns the terrain (and optional water, snow, allowed-area and
//! Occupied masks) into a density mask using Gaea's three factors: growth /
//! health, inhibitors and dead zones. Points are then sampled from the density
//! with a minimum spacing in metres. Candidate points sit on a grid of cells
//! fixed in the world, and every random choice is seeded from a cell's world
//! coordinates, so points don't move when the resolution changes.

use std::sync::Arc;

use rayon::prelude::*;
use terrain_core::error::{CoreError, Result};
use terrain_core::ops::{gaussian_blur, slope_degrees, smoothstep, soft_range};
use terrain_core::seed::{derive, mix64};
use terrain_core::{
    ColorGrid, EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, Point, PointSet, PortDef,
    PortType, Value,
};

use crate::common::{lerp, seed_param};
use crate::noise::basis::{self, Basis};

/// Most candidate cells a population may sample (one per `spacing / √2`
/// square of the world): keeps memory for the points pass under ~350 MB.
pub const MAX_POINT_CELLS: u64 = 40_000_000;

/// Scale over which the terrain is compared with its surroundings to find
/// valleys (wetter) and peaks and ridges, in metres.
const RELIEF_SIGMA_M: f64 = 120.0;
/// Height below (or above) the surroundings that counts as fully a valley
/// (or a ridge), in metres.
const RELIEF_RANGE_M: f32 = 40.0;

// ---- shared parameters ------------------------------------------------------

fn species_param(default: &str) -> ParamDef {
    ParamDef::text("species", "Species", default).describe(
        "Name written with each point in exported point files (e.g. scots_pine), so import scripts \
         can pick the matching mesh.",
    )
}

fn point_params(spacing: f64, scale_min: f64, scale_max: f64) -> Vec<ParamDef> {
    vec![
        ParamDef::float("spacing_m", "Spacing", spacing, 0.25, 1000.0)
            .unit("m")
            .describe("Minimum distance between two points, in metres."),
        ParamDef::float("rotation_min_deg", "Rotation min", 0.0, 0.0, 360.0)
            .unit("°")
            .describe("Each point turns by a random angle between Rotation min and max."),
        ParamDef::float("rotation_max_deg", "Rotation max", 360.0, 0.0, 360.0).unit("°"),
        ParamDef::float("scale_min", "Scale min", scale_min, 0.01, 100.0)
            .describe("Each point gets a random scale between Scale min and max (1 = the mesh as modelled)."),
        ParamDef::float("scale_max", "Scale max", scale_max, 0.01, 100.0),
    ]
}

fn patch_params(patches: f64, size: f64) -> Vec<ParamDef> {
    vec![
        ParamDef::float("patches", "Patches", patches, 0.0, 1.0)
            .describe("Groups plants into clumps and clearings. 0 = even cover."),
        ParamDef::metres("patch_size_m", "Patch size", size, 5.0, 20_000.0)
            .describe("Typical size of clumps and clearings, in metres."),
    ]
}

/// A random number in 0..1 for candidate cell `(ci, cj)`, stream `k`.
#[inline]
fn cell_random(seed: u64, ci: i64, cj: i64, k: u64) -> f64 {
    let h = mix64(
        mix64(seed ^ (ci as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            ^ (cj as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ k.wrapping_mul(0xd6e8_feb8_6659_fd93),
    );
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Perlin fBm remapped to about 0..1.
#[inline]
fn noise01(x: f64, y: f64, size_m: f64, seed: u64, octaves: u32) -> f32 {
    let n = basis::fbm(Basis::Perlin, x / size_m, y / size_m, seed, octaves, 2.0, 0.5);
    (0.5 + 0.5 * n).clamp(0.0, 1.0) as f32
}

/// Patch factor: 1 with `patches` = 0, clumps and clearings as it rises.
fn patch_factor(amount: f32, x: f64, y: f64, size_m: f64, seed: u64) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let n = noise01(x, y, size_m, derive(seed, 1), 3);
    lerp(1.0, smoothstep(0.35, 0.65, n), amount)
}

/// Height relative to the surroundings: > 0 on peaks and ridges, < 0 in
/// valleys, in units of [`RELIEF_RANGE_M`].
fn relief(height: &Grid) -> Grid {
    let blurred = gaussian_blur(height, RELIEF_SIGMA_M);
    height.map_indexed(|i, h| (h - blurred.data[i]) / RELIEF_RANGE_M)
}

// ---- point sampling -----------------------------------------------------------

/// Settings for [`scatter`].
pub struct Scatter<'a> {
    pub spacing_m: f64,
    pub rotation_deg: (f32, f32),
    pub scale: (f32, f32),
    pub species: &'a str,
    pub seed: u64,
}

/// Poisson-disk style points from a density mask: at most one point per
/// world-fixed cell of `spacing / √2`, kept with probability equal to the
/// density there and moved (up to three tries) to stay `spacing` metres
/// from every other point. Cells are handled in nine interleaved phases so
/// cells handled together are too far apart to conflict: the result is the
/// same for any number of threads. Heights come from `height`.
pub fn scatter(density: &Grid, height: &Grid, s: &Scatter, cancel: &dyn Fn() -> bool) -> Result<PointSet> {
    let spec = density.spec;
    let r = s.spacing_m.max(0.01);
    let cell = r / std::f64::consts::SQRT_2;
    let x_end = spec.origin_m[0] + spec.extent_m[0];
    let y_end = spec.origin_m[1] + spec.extent_m[1];
    let (ci0, cj0) = (
        (spec.origin_m[0] / cell).floor() as i64,
        (spec.origin_m[1] / cell).floor() as i64,
    );
    let (ci1, cj1) = ((x_end / cell).floor() as i64, (y_end / cell).floor() as i64);
    let (nx, ny) = ((ci1 - ci0 + 1) as usize, (cj1 - cj0 + 1) as usize);
    if nx as u64 * ny as u64 > MAX_POINT_CELLS {
        return Err(CoreError::InvalidParam {
            param: "spacing_m".into(),
            reason: format!(
                "a spacing of {r} m needs {:.0} million candidate points over this world; the most is {} \
                 million. Raise the spacing.",
                (nx * ny) as f64 / 1.0e6,
                MAX_POINT_CELLS / 1_000_000
            ),
        });
    }
    // Offset of each cell's point inside the cell (0..1 each way); NaN = none.
    let mut slots = vec![[f32::NAN; 2]; nx * ny];
    let r2 = r * r;
    for phase in 0..9usize {
        if cancel() {
            return Err(CoreError::Cancelled);
        }
        let (px, py) = (phase % 3, phase / 3);
        let slots_ref = &slots;
        let placed: Vec<(usize, [f32; 2])> = (py..ny)
            .step_by(3)
            .collect::<Vec<_>>()
            .par_iter()
            .flat_map_iter(|&ly| {
                (px..nx).step_by(3).filter_map(move |lx| {
                    let (ci, cj) = (ci0 + lx as i64, cj0 + ly as i64);
                    let (cx, cy) = ((ci as f64 + 0.5) * cell, (cj as f64 + 0.5) * cell);
                    let d = density.sample_bilinear_m(
                        cx.clamp(spec.origin_m[0], x_end),
                        cy.clamp(spec.origin_m[1], y_end),
                    );
                    if cell_random(s.seed, ci, cj, 0) >= d as f64 {
                        return None;
                    }
                    for attempt in 0..3u64 {
                        let fx = cell_random(s.seed, ci, cj, 1 + 2 * attempt);
                        let fy = cell_random(s.seed, ci, cj, 2 + 2 * attempt);
                        let (x, y) = ((ci as f64 + fx) * cell, (cj as f64 + fy) * cell);
                        if x < spec.origin_m[0] || x > x_end || y < spec.origin_m[1] || y > y_end {
                            continue;
                        }
                        let clear = (-2i64..=2).all(|dj| {
                            (-2i64..=2).all(|di| {
                                if (di == 0 && dj == 0) || (di.abs() == 2 && dj.abs() == 2) {
                                    return true;
                                }
                                let (ni, nj) = (lx as i64 + di, ly as i64 + dj);
                                if ni < 0 || nj < 0 || ni >= nx as i64 || nj >= ny as i64 {
                                    return true;
                                }
                                let o = slots_ref[nj as usize * nx + ni as usize];
                                if o[0].is_nan() {
                                    return true;
                                }
                                let ox = ((ci + di) as f64 + o[0] as f64) * cell;
                                let oy = ((cj + dj) as f64 + o[1] as f64) * cell;
                                (ox - x) * (ox - x) + (oy - y) * (oy - y) >= r2
                            })
                        });
                        if clear {
                            return Some((ly * nx + lx, [fx as f32, fy as f32]));
                        }
                    }
                    None
                })
            })
            .collect();
        for (i, o) in placed {
            slots[i] = o;
        }
    }

    let mut points = PointSet::new(spec, s.species);
    let (rot0, rot1) = s.rotation_deg;
    let (sc0, sc1) = s.scale;
    for (idx, o) in slots.iter().enumerate() {
        if o[0].is_nan() {
            continue;
        }
        let (ci, cj) = (ci0 + (idx % nx) as i64, cj0 + (idx / nx) as i64);
        let x = (ci as f64 + o[0] as f64) * cell;
        let y = (cj as f64 + o[1] as f64) * cell;
        let rotation = lerp(rot0, rot1, cell_random(s.seed, ci, cj, 10) as f32).rem_euclid(360.0);
        points.push(Point {
            x_m: x as f32,
            y_m: y as f32,
            z_m: height.sample_bilinear_m(x, y),
            rotation_deg: rotation,
            scale: lerp(sc0, sc1, cell_random(s.seed, ci, cj, 11) as f32),
            species: 0,
        });
    }
    Ok(points)
}

fn scatter_from_params(ctx: &EvalContext, density: &Grid, height: &Grid) -> Result<PointSet> {
    let species = ctx.text("species");
    let species = if species.is_empty() {
        "plant".to_string()
    } else {
        species
    };
    let (s0, s1) = (ctx.f32("scale_min"), ctx.f32("scale_max"));
    scatter(
        density,
        height,
        &Scatter {
            spacing_m: ctx.f64("spacing_m"),
            rotation_deg: (ctx.f32("rotation_min_deg"), ctx.f32("rotation_max_deg")),
            scale: (s0.min(s1), s0.max(s1)),
            species: &species,
            seed: derive(ctx.seed, 7),
        },
        &|| ctx.is_cancelled(),
    )
}

// ---- populations: Trees, Shrubs, Grass -------------------------------------------

/// Default settings that make Trees, Shrubs and Grass differ.
struct Defaults {
    type_id: &'static str,
    label: &'static str,
    description: &'static str,
    species: &'static str,
    spacing_m: f64,
    scale: (f64, f64),
    slope_max_deg: f64,
    height_max_m: f64,
    water_preference: f64,
    avoid_peaks: f64,
    dead_zones: f64,
    patches: f64,
    patch_size_m: f64,
    spread_m: f64,
}

/// One plant population (Trees, Shrubs or Grass).
pub struct Population {
    schema: NodeSchema,
}

impl Population {
    fn new(d: Defaults) -> Self {
        let mut params =
            vec![
            species_param(d.species),
            ParamDef::float("coverage", "Coverage", 0.8, 0.0, 1.0)
                .describe("Density where conditions are ideal. 1 packs plants as closely as Spacing allows.")
                .drivable(),
            // Growth / health
            ParamDef::float("health", "Health", 0.8, 0.0, 1.0).describe(
                "How well the plants cope with poor ground. Lower health leaves only the best spots covered.",
            ),
            ParamDef::float("water_preference", "Water preference", d.water_preference, -1.0, 1.0).describe(
                "Above 0 the plants seek water, below 0 they avoid it. Water is the Water input (e.g. \
                 a Wetness mask), or valleys if it isn't connected.",
            ),
            ParamDef::metres("spread_m", "Spread", d.spread_m, 0.0, 2000.0)
                .describe("How far plants spread beyond the ground that suits them, in metres."),
        ];
        params.extend(patch_params(d.patches, d.patch_size_m));
        params.extend([
            ParamDef::float("randomness", "Randomness", 0.3, 0.0, 1.0)
                .describe("Fine, random thinning of the cover."),
            // Inhibitors
            ParamDef::float("slope_min_deg", "Slope min", 0.0, 0.0, 90.0)
                .unit("°")
                .describe("Plants grow on slopes between Slope min and max."),
            ParamDef::float("slope_max_deg", "Slope max", d.slope_max_deg, 0.0, 90.0).unit("°"),
            ParamDef::float("slope_falloff_deg", "Slope falloff", 6.0, 0.0, 45.0)
                .unit("°")
                .describe("How gradually cover fades outside the slope range."),
            ParamDef::metres("height_min_m", "Altitude min", -10_000.0, -10_000.0, 20_000.0)
                .describe("Plants grow between Altitude min and max, in metres."),
            ParamDef::metres(
                "height_max_m",
                "Altitude max",
                d.height_max_m,
                -10_000.0,
                20_000.0,
            ),
            ParamDef::metres("height_falloff_m", "Altitude falloff", 100.0, 0.0, 5000.0)
                .describe("How gradually cover fades outside the altitude range (the tree line)."),
            ParamDef::float("avoid_peaks", "Avoid peaks", d.avoid_peaks, 0.0, 1.0)
                .describe("Thins cover on exposed peaks and ridge crests."),
            ParamDef::float("snow_tolerance", "Snow tolerance", 0.2, 0.0, 1.0)
                .describe("How much snow (Snow input) the plants survive. 0 = none."),
            ParamDef::choice(
                "occupied_mode",
                "Occupied",
                "avoid",
                &[
                    ("avoid", "Avoid"),
                    ("intermingle", "Intermingle"),
                    ("near", "Grow near"),
                ],
            )
            .describe(
                "How this population treats ground taken by earlier ones (Occupied input): keep out, \
                 share it thinly, or cluster around its edges.",
            ),
            ParamDef::float("occupied_strength", "Occupied strength", 1.0, 0.0, 1.0),
            // Dead zones
            ParamDef::float("dead_zones", "Dead zones", d.dead_zones, 0.0, 1.0).describe(
                "Clears scree slopes, their run-outs and snow-slide paths. Also an output, for placing \
                 rocks.",
            ),
            ParamDef::float("dead_slope_deg", "Dead zone slope", 38.0, 0.0, 90.0)
                .unit("°")
                .describe("Slopes steeper than this become dead zones."),
        ]);
        params.extend(point_params(d.spacing_m, d.scale.0, d.scale.1));
        params.push(seed_param());
        Self {
            schema: NodeSchema {
                type_id: d.type_id.into(),
                type_version: 1,
                label: d.label.into(),
                category: "Vegetation".into(),
                description: d.description.into(),
                inputs: vec![
                    PortDef::new("in", "Terrain", PortType::Heightfield),
                    PortDef::new("water", "Water", PortType::Mask).optional(),
                    PortDef::new("snow", "Snow", PortType::Mask).optional(),
                    PortDef::new("area", "Allowed area", PortType::Mask).optional(),
                    PortDef::new("occupied", "Occupied", PortType::Mask).optional(),
                ],
                outputs: vec![
                    PortDef::new("density", "Density", PortType::Mask),
                    PortDef::new("points", "Points", PortType::PointSet),
                    PortDef::new("dead_zones", "Dead zones", PortType::Mask),
                    PortDef::new("occupied", "Occupied", PortType::Mask),
                    PortDef::new("water", "Water influence", PortType::Mask),
                ],
                params,
                gpu: false,
            },
        }
    }

    pub fn trees() -> Self {
        Self::new(Defaults {
            type_id: "vegetation.trees",
            label: "Trees",
            description: "A tree population: where trees grow (Density) and one point per tree \
                          (Points), from slope, altitude, water and earlier populations (Occupied). \
                          Dead zones marks scree and slide paths. Occupied is the ground taken (all \
                          of it from a density of 0.5) added to the Occupied input, for chaining \
                          populations.",
            species: "tree",
            spacing_m: 7.0,
            scale: (0.8, 1.2),
            slope_max_deg: 35.0,
            height_max_m: 1400.0,
            water_preference: 0.2,
            avoid_peaks: 0.4,
            dead_zones: 0.5,
            patches: 0.4,
            patch_size_m: 150.0,
            spread_m: 20.0,
        })
    }

    pub fn shrubs() -> Self {
        Self::new(Defaults {
            type_id: "vegetation.shrubs",
            label: "Shrubs",
            description: "A shrub population: bushes and undergrowth, denser and hardier than trees. \
                          Outputs as for Trees.",
            species: "shrub",
            spacing_m: 3.0,
            scale: (0.7, 1.3),
            slope_max_deg: 42.0,
            height_max_m: 1800.0,
            water_preference: 0.1,
            avoid_peaks: 0.2,
            dead_zones: 0.3,
            patches: 0.5,
            patch_size_m: 60.0,
            spread_m: 10.0,
        })
    }

    pub fn grass() -> Self {
        Self::new(Defaults {
            type_id: "vegetation.grass",
            label: "Grass",
            description: "A grass population: meadows and ground cover, mostly used as a Density mask \
                          for engine grass painting. Points are clumps. Outputs as for Trees.",
            species: "grass",
            spacing_m: 2.0,
            scale: (0.6, 1.4),
            slope_max_deg: 45.0,
            height_max_m: 2200.0,
            water_preference: 0.3,
            avoid_peaks: 0.0,
            dead_zones: 0.2,
            patches: 0.3,
            patch_size_m: 40.0,
            spread_m: 5.0,
        })
    }
}

impl NodeKind for Population {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }

    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let height = ctx.input_grid("in")?.clone();
        let spec = ctx.spec;
        let slope = slope_degrees(&height);
        let relief = relief(&height);
        let optional = |key: &str| ctx.input(key).map(|v| v.grid().clone());
        let (water_in, snow, area, occupied) = (
            optional("water"),
            optional("snow"),
            optional("area"),
            optional("occupied"),
        );
        ctx.report_progress(0.1);

        // Water the plants respond to: the input, or valleys.
        let water = match &water_in {
            Some(w) => w.map(|v| v.clamp(0.0, 1.0)),
            None => relief.map(|r| (0.5 - 0.5 * r).clamp(0.0, 1.0)),
        };

        // Inhibitors and water preference on the terrain, softened by Spread.
        let (smin, smax, sf) = (
            ctx.f32("slope_min_deg"),
            ctx.f32("slope_max_deg"),
            ctx.f32("slope_falloff_deg"),
        );
        let (hmin, hmax, hf) = (
            ctx.f32("height_min_m"),
            ctx.f32("height_max_m"),
            ctx.f32("height_falloff_m"),
        );
        let pref = ctx.f32("water_preference");
        let peaks = ctx.f32("avoid_peaks");
        let snow_tol = ctx.f32("snow_tolerance");
        let habitat = Grid::from_fn_indexed(spec, |i, _, _| {
            let w = water.data[i];
            let water_factor = if pref >= 0.0 {
                lerp(1.0, w, pref)
            } else {
                lerp(1.0, 1.0 - w, -pref)
            };
            let snow_factor = snow
                .as_ref()
                .map_or(1.0, |s| 1.0 - s.data[i].clamp(0.0, 1.0) * (1.0 - snow_tol));
            soft_range(slope.data[i], smin, smax, sf)
                * soft_range(height.data[i], hmin, hmax, hf)
                * water_factor
                * snow_factor
                * (1.0 - peaks * relief.data[i].clamp(0.0, 1.0))
        });
        let spread = ctx.f64("spread_m");
        let habitat = if spread > 0.0 {
            gaussian_blur(&habitat, spread * 0.5)
        } else {
            habitat
        };
        ctx.report_progress(0.3);

        // Dead zones: steep scree, its run-out below, and snow-slide paths.
        let dead_amount = ctx.f32("dead_zones");
        let dead_slope = ctx.f32("dead_slope_deg");
        let dead = if dead_amount > 0.0 {
            let steep = slope.map(|s| smoothstep(dead_slope - 6.0, dead_slope + 6.0, s));
            let runout = gaussian_blur(&steep, 25.0);
            Grid::from_fn_indexed(spec, |i, x, y| {
                let slide = snow.as_ref().map_or(0.0, |s| {
                    s.data[i].clamp(0.0, 1.0) * smoothstep(25.0, 40.0, slope.data[i])
                });
                let raw = steep.data[i]
                    .max((runout.data[i] * 1.6).min(1.0) * 0.8)
                    .max(slide);
                let ragged = noise01(x, y, 80.0, derive(ctx.seed, 3), 2);
                (dead_amount * raw * lerp(0.8, 1.5, ragged)).clamp(0.0, 1.0)
            })
        } else {
            Grid::filled(spec, 0.0)
        };

        // Earlier populations.
        let mode = ctx.choice("occupied_mode");
        let strength = ctx.f32("occupied_strength");
        let near = match (&occupied, mode.as_str()) {
            (Some(o), "near") => Some(gaussian_blur(o, 40.0)),
            _ => None,
        };
        ctx.report_progress(0.5);

        // Growth / health, patches, randomness, allowed area.
        let coverage = ctx.field("coverage");
        let health = ctx.f32("health");
        let patches = ctx.f32("patches");
        let patch_size = ctx.f64("patch_size_m");
        let randomness = ctx.f32("randomness");
        let fine_size = (ctx.f64("spacing_m") * 4.0).max(4.0);
        let seed = ctx.seed;
        let density = Grid::from_fn_indexed(spec, |i, x, y| {
            let h = habitat.data[i];
            let healthy = if health >= 1.0 {
                h
            } else {
                ((h - (1.0 - health)) / health.max(0.05)).clamp(0.0, 1.0)
            };
            let fine = if randomness > 0.0 {
                lerp(1.0, noise01(x, y, fine_size, derive(seed, 2), 2), randomness)
            } else {
                1.0
            };
            let occ = occupied.as_ref().map_or(1.0, |o| {
                let o = o.data[i].clamp(0.0, 1.0);
                match mode.as_str() {
                    "intermingle" => 1.0 - 0.5 * strength * o,
                    "near" => {
                        let around = near.as_ref().map_or(0.0, |g| g.data[i]);
                        lerp(1.0, ((around * 2.0).min(1.0)) * (1.0 - o), strength)
                    }
                    _ => 1.0 - strength * o,
                }
            });
            let allowed = area.as_ref().map_or(1.0, |a| a.data[i].clamp(0.0, 1.0));
            (coverage.at(i)
                * healthy
                * patch_factor(patches, x, y, patch_size, seed)
                * fine
                * allowed
                * occ
                * (1.0 - dead.data[i]))
                .clamp(0.0, 1.0)
        });
        ctx.report_progress(0.6);

        let points = scatter_from_params(ctx, &density, &height)?;
        let occupied_out = density.map_indexed(|i, d| {
            let o = occupied.as_ref().map_or(0.0, |o| o.data[i].clamp(0.0, 1.0));
            // Ground counts as fully taken from a density of 0.5.
            1.0 - (1.0 - o) * (1.0 - (d * 2.0).min(1.0))
        });
        Ok(Outputs::from([
            ("density".to_string(), Value::Mask(Arc::new(density))),
            ("points".to_string(), Value::Points(Arc::new(points))),
            ("dead_zones".to_string(), Value::Mask(Arc::new(dead))),
            ("occupied".to_string(), Value::Mask(Arc::new(occupied_out))),
            ("water".to_string(), Value::Mask(Arc::new(water))),
        ]))
    }
}

// ---- Debris / Rocks ---------------------------------------------------------------

/// Rocks and debris: gathers below steep ground (or on a talus mask such as
/// Thermal Erosion's Debris output) and in dead zones.
pub struct Debris {
    schema: NodeSchema,
}

impl Default for Debris {
    fn default() -> Self {
        let mut params = vec![
            species_param("rock"),
            ParamDef::float("coverage", "Coverage", 0.8, 0.0, 1.0)
                .describe("Density where debris gathers most.")
                .drivable(),
            ParamDef::float("slope_min_deg", "Shedding slope", 32.0, 0.0, 90.0)
                .unit("°")
                .describe("Slopes steeper than this shed rocks (when no Talus input is connected)."),
            ParamDef::metres("reach_m", "Reach", 60.0, 0.0, 2000.0)
                .describe("How far rocks roll out from steep ground, in metres (without a Talus input)."),
            ParamDef::float("talus_weight", "Talus", 1.0, 0.0, 1.0)
                .describe("How much debris lies on talus (the Talus input, or ground below steep slopes)."),
            ParamDef::float("dead_zone_weight", "Dead zones", 1.0, 0.0, 1.0)
                .describe("How much debris lies in dead zones (Dead zones input)."),
        ];
        params.extend(patch_params(0.4, 50.0));
        params.extend(point_params(4.0, 0.4, 1.8));
        params.push(seed_param());
        Self {
            schema: NodeSchema {
                type_id: "vegetation.debris".into(),
                type_version: 1,
                label: "Debris / Rocks".into(),
                category: "Vegetation".into(),
                description: "Scatters rocks where they collect: talus below cliffs and steep slopes (or \
                              the Talus input, e.g. Thermal Erosion's Debris) and a population's Dead \
                              zones. Outputs a Density mask and one point per rock."
                    .into(),
                inputs: vec![
                    PortDef::new("in", "Terrain", PortType::Heightfield),
                    PortDef::new("talus", "Talus", PortType::Mask).optional(),
                    PortDef::new("dead_zones", "Dead zones", PortType::Mask).optional(),
                    PortDef::new("area", "Allowed area", PortType::Mask).optional(),
                ],
                outputs: vec![
                    PortDef::new("density", "Density", PortType::Mask),
                    PortDef::new("points", "Points", PortType::PointSet),
                ],
                params,
                gpu: false,
            },
        }
    }
}

impl NodeKind for Debris {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }

    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let height = ctx.input_grid("in")?.clone();
        let spec = ctx.spec;
        let talus = match ctx.input("talus") {
            Some(t) => t.grid().map(|v| v.clamp(0.0, 1.0)),
            None => {
                let smin = ctx.f32("slope_min_deg");
                let steep = slope_degrees(&height).map(|s| smoothstep(smin - 4.0, smin + 10.0, s));
                let reach = ctx.f64("reach_m");
                let rolled = if reach > 0.0 {
                    gaussian_blur(&steep, reach * 0.5)
                } else {
                    steep.clone()
                };
                // Mostly at the foot of steep ground, a little on it.
                rolled.map_indexed(|i, r| ((r * 1.8).min(1.0) * (1.0 - 0.7 * steep.data[i])).clamp(0.0, 1.0))
            }
        };
        let dead = ctx.input("dead_zones").map(|v| v.grid().clone());
        let area = ctx.input("area").map(|v| v.grid().clone());
        let (tw, dw) = (ctx.f32("talus_weight"), ctx.f32("dead_zone_weight"));
        let coverage = ctx.field("coverage");
        let (patches, patch_size, seed) = (ctx.f32("patches"), ctx.f64("patch_size_m"), ctx.seed);
        let density = Grid::from_fn_indexed(spec, |i, x, y| {
            let d = dead.as_ref().map_or(0.0, |g| g.data[i].clamp(0.0, 1.0));
            let allowed = area.as_ref().map_or(1.0, |a| a.data[i].clamp(0.0, 1.0));
            ((talus.data[i] * tw).max(d * dw)
                * coverage.at(i)
                * patch_factor(patches, x, y, patch_size, seed)
                * allowed)
                .clamp(0.0, 1.0)
        });
        let points = scatter_from_params(ctx, &density, &height)?;
        Ok(Outputs::from([
            ("density".to_string(), Value::Mask(Arc::new(density))),
            ("points".to_string(), Value::Points(Arc::new(points))),
        ]))
    }
}

// ---- Pack Masks (RGBA) ----------------------------------------------------------

/// Four masks in the R, G, B and A channels of one image, e.g. four
/// population densities for an engine's foliage or grass tool.
pub struct PackMasks {
    schema: NodeSchema,
}

impl Default for PackMasks {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "output.pack_masks".into(),
                type_version: 1,
                label: "Pack Masks (RGBA)".into(),
                category: "Output".into(),
                description: "Packs up to four masks into the red, green, blue and alpha channels of one \
                              image, values unchanged (not normalised, unlike Splat Map). Unconnected \
                              channels are 0. Export as PNG 8 for engines."
                    .into(),
                inputs: ["r", "g", "b", "a"]
                    .iter()
                    .zip(["Red", "Green", "Blue", "Alpha"])
                    .map(|(k, l)| PortDef::new(k, l, PortType::Mask).optional())
                    .collect(),
                outputs: vec![PortDef::new("out", "Packed (RGBA)", PortType::ColorMap)],
                params: vec![],
                gpu: false,
            },
        }
    }
}

impl NodeKind for PackMasks {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }

    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let channels: Vec<Option<Arc<Grid>>> = ["r", "g", "b", "a"]
            .iter()
            .map(|k| ctx.input(k).map(|v| v.grid().clone()))
            .collect();
        let packed = ColorGrid::from_fn_indexed(ctx.spec, |i, _, _| {
            std::array::from_fn(|c| channels[c].as_ref().map_or(0.0, |g| g.data[i].clamp(0.0, 1.0)))
        });
        Ok(Outputs::from([(
            "out".to_string(),
            Value::ColorMap(Arc::new(packed)),
        )]))
    }
}
