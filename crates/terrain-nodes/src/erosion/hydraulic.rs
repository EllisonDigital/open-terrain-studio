use super::*;
use crate::hydro::{FLOW_HI_M2, FLOW_LO_M2, flat_jitter, log_mask, resample, route, simulation_spec, snap01};
use terrain_core::{GridSpec, NodeKind};

/// Fluvial erosion over geological time: rivers cut valleys in proportion to
/// how much water drains through them (the stream-power law), and carry the
/// sediment downstream until they slow down and drop it.
///
/// Each time step (clean-room, from the published methods):
/// 1. Route flow: Priority-Flood+ε (Barnes, Lehman & Mulla 2014) fills pits
///    for routing only, and its pop order is a downstream-first ordering. Every
///    cell drains to its steepest downhill neighbour (8 directions); cells on
///    the world's edge are outlets.
/// 2. Discharge Q (m³/yr): rain × cell area, accumulated from the ridges down,
///    losing a share to evaporation per kilometre travelled.
/// 3. Erosion E = K·Q^0.5·S, solved implicitly in downstream order
///    (Braun & Willett 2013), so any time step is stable. Erosion doesn't
///    depend on the sediment already carried (detachment-limited, Howard
///    1994); compare the sediment-flux models of Whipple & Tucker (2002) and
///    Davy & Lague (2009), where a full river stops cutting its bed.
/// 4. Sediment is carried downstream; where it exceeds the flow's capacity
///    (proportional to its stream power) the excess settles, filling lakes and
///    flats first.
/// 5. Hillslope creep: linear diffusion (Culling 1960), applied exactly as a
///    Gaussian blur with σ = √(2·D·dt) metres (the heat kernel), so it is the
///    same at any resolution.
///
/// Every stage is sequential in a fixed order, or parallel over independent
/// cells, so results are identical for any number of threads.
pub struct Hydraulic {
    schema: NodeSchema,
}
impl Default for Hydraulic {
    fn default() -> Self {
        Self {
            schema: schema(
                "simulate.hydraulic",
                "Hydraulic Erosion",
                "Rivers carve branching valleys over geological time and drop their sediment where they slow \
                 down. Strength limits where it acts; hardness resists wear. Water leaves at the world's edges.",
                &[
                    ("flow", "Flow"),
                    ("wear", "Wear"),
                    ("deposition", "Deposition"),
                    ("sediment", "Sediment"),
                ],
                vec![
                    ParamDef::float("duration_kyr", "Duration", 1000.0, 0.0, 5000.0)
                        .unit("kyr")
                        .describe("Time simulated, in thousands of years. Zero leaves the terrain unchanged."),
                    ParamDef::float("rainfall_m_yr", "Rainfall", 1.0, 0.0, 10.0)
                        .unit("m/yr")
                        .describe("Yearly rainfall. More rain makes bigger rivers that cut faster."),
                    ParamDef::float("rock_softness", "Rock softness", 0.5, 0.0, 1.0)
                        .describe("How easily rivers wear the rock; multiplied by 1 - hardness."),
                    ParamDef::float("sediment_capacity", "Sediment capacity", 1.5, 0.0, 10.0).describe(
                        "How much sediment a river can carry, relative to what it erodes. \
                         Lower values leave more sediment in valleys.",
                    ),
                    ParamDef::float("deposition_rate", "Deposition", 0.5, 0.0, 1.0).describe(
                        "Share of the excess sediment that settles per 100 m of flow. 0 = never settles.",
                    ),
                    ParamDef::float("evaporation", "Evaporation", 0.02, 0.0, 1.0)
                        .describe("Share of the water lost per kilometre it flows."),
                    ParamDef::float("downcutting", "Downcutting", 1.0, 0.0, 4.0)
                        .describe("Multiplier on river erosion. Zero stops new wear."),
                    ParamDef::metres("detail_m", "Detail size", 8.0, 1.0, 1000.0).describe(
                        "Cell size the rivers are simulated at, in metres: about the width of the smallest \
                         valleys. Smaller is slower. The result is the same at any preview or build resolution.",
                    ),
                    ParamDef::float("creep_m2_yr", "Hillslope creep", 0.002, 0.0, 1.0)
                        .unit("m²/yr")
                        .describe("Soil creep that rounds hillslopes and smooths small rills between rivers."),
                ],
            ),
        }
    }
}

/// Erodibility at rock softness 1 (Q in m³/yr, time in years).
const K_MAX: f64 = 1.0e-5;
/// Discharge exponent m (slope exponent n = 1).
const M: f64 = 0.5;
/// Longest time step, in years. Implicit steps are stable at any length; this
/// bounds how far routing lags behind the changing surface.
const MAX_DT_YR: f64 = 25_000.0;
/// Sediment mask: sediment load (m³/yr) on a log scale.
const SED_LO: f64 = 1.0;
const SED_HI: f64 = 1.0e5;
/// Wear and deposition masks: net depth (m) at half brightness.
const WEAR_HALF_M: f64 = 30.0;
const DEPOSITION_HALF_M: f64 = 10.0;

/// Everything the simulation produces, on the simulation grid.
struct Simulated {
    height: Vec<f64>,
    /// Discharge through each cell in the last step, m³/yr.
    discharge: Vec<f64>,
    /// Sediment load through each cell in the last step, m³/yr.
    load: Vec<f64>,
}

/// Run the time steps on `spec` from heights `h`. `strength` and `softness`
/// (1 - hardness) are per cell of `spec`.
fn simulate(
    ctx: &EvalContext,
    spec: GridSpec,
    mut h: Vec<f64>,
    strength: &[f32],
    softness: &[f32],
) -> Result<Simulated> {
    let (w, ht) = (spec.width as usize, spec.height as usize);
    let [dx, dy] = spec.cell_size_m();
    let cell_area = dx * dy;
    let n = h.len();
    let duration_yr = ctx.f64("duration_kyr") * 1000.0;
    let steps = (duration_yr / MAX_DT_YR).ceil().max(1.0) as usize;
    let dt = duration_yr / steps as f64;
    let rain = ctx.f64("rainfall_m_yr");
    let k = K_MAX * ctx.f64("rock_softness") * ctx.f64("downcutting");
    let capacity_factor = ctx.f64("sediment_capacity");
    let unsettled_per_100m = 1.0 - ctx.f64("deposition_rate");
    let kept_per_m = 1.0 - ctx.f64("evaporation") / 1000.0;
    let creep_sigma_m = (2.0 * ctx.f64("creep_m2_yr") * dt).sqrt();
    let weight: Vec<f64> = (0..n).map(|i| (strength[i] * softness[i]) as f64).collect();
    let jitter = flat_jitter(n);

    let mut q = vec![0.0f64; n];
    let mut load = vec![0.0f64; n];
    if duration_yr <= 0.0 {
        return Ok(Simulated {
            height: h,
            discharge: q,
            load,
        });
    }
    for step in 0..steps {
        check_cancel(ctx)?;
        let r = route(&h, &jitter, w, ht, dx, dy);
        // Per-link factors: links have only four lengths.
        let kept = r.per_link(|d| libm::pow(kept_per_m, d));
        let unsettled = r.per_link(|d| libm::pow(unsettled_per_100m, d / 100.0));

        // Discharge, from the ridges down.
        q.par_iter_mut().for_each(|v| *v = rain * cell_area);
        for &i in r.stack.iter().rev() {
            let j = r.receiver[i];
            if j != i {
                q[j] += q[i] * kept[r.link[i] as usize];
            }
        }
        check_cancel(ctx)?;

        // Stream power's discharge term, computed once for both passes below.
        let q_m: Vec<f64> = q.par_iter().map(|&v| libm::pow(v, M)).collect();

        // Implicit stream-power erosion, downstream first: each cell is solved
        // against its receiver's already-updated height.
        let before = h.clone();
        for &i in &r.stack {
            let j = r.receiver[i];
            if j == i || weight[i] == 0.0 || before[i] <= h[j] {
                continue; // Outlet, protected, or in a pit (a lake).
            }
            let f = k * weight[i] * dt * q_m[i] / r.distance(i);
            h[i] = (before[i] + f * h[j]) / (1.0 + f);
        }
        check_cancel(ctx)?;

        // Sediment: carried from the ridges down, settling where the load
        // exceeds what the flow can carry.
        load.par_iter_mut().for_each(|v| *v = 0.0);
        for &i in r.stack.iter().rev() {
            let j = r.receiver[i];
            let eroded = (before[i] - h[i]).max(0.0);
            let carried = load[i] + eroded * cell_area;
            let mut settled = 0.0;
            if j != i && carried > 0.0 {
                let slope = ((h[i] - h[j]) / r.distance(i)).max(0.0);
                let capacity = capacity_factor * K_MAX * dt * q_m[i] * slope * cell_area;
                if carried > capacity {
                    let share = 1.0 - unsettled[r.link[i] as usize];
                    // Fill lakes up to their spill level; elsewhere at most
                    // halve the drop to the next cell.
                    let lake = (r.filled[i] - h[i]).max(0.0);
                    let room = lake.max(0.5 * (h[i] - h[j]).max(0.0)) * cell_area;
                    settled = ((carried - capacity) * share * strength[i] as f64).min(room);
                    h[i] += settled / cell_area;
                }
            }
            let passed = carried - settled;
            if j != i {
                load[j] += passed;
            }
            // Keep this step's load through the cell, per year, for the mask.
            load[i] = passed / dt;
        }
        check_cancel(ctx)?;

        // Hillslope creep, weighted so protected or hard ground keeps its shape.
        if creep_sigma_m > 0.0 {
            let grid = Grid {
                spec,
                data: h.iter().map(|&v| v as f32).collect(),
            };
            let smooth = terrain_core::ops::gaussian_blur(&grid, creep_sigma_m);
            h.par_iter_mut().enumerate().for_each(|(i, v)| {
                if weight[i] > 0.0 {
                    *v += (smooth.data[i] - grid.data[i]) as f64 * weight[i];
                }
            });
        }
        ctx.report_progress((step + 1) as f32 / steps as f32 * 0.95);
    }
    Ok(Simulated {
        height: h,
        discharge: q,
        load,
    })
}

impl NodeKind for Hydraulic {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn world_spec(&self, ctx: &EvalContext) -> Option<GridSpec> {
        Some(simulation_spec(ctx.spec.whole(), ctx.f64("detail_m")))
    }
    fn finish_tile(&self, ctx: &EvalContext, world: &terrain_core::WorldPass) -> Result<Outputs> {
        let mut outputs = Outputs::new();
        for port in &self.schema.outputs {
            if let Some(v) = world.outputs.get(&port.key) {
                let how = self.upsample(port);
                outputs.insert(
                    port.key.clone(),
                    terrain_core::node::upsample_value(ctx, world, v, &how)?,
                );
            }
        }
        // As untiled: ground with zero strength keeps its input exactly.
        if let (Some(mask), Some(Value::Heightfield(h))) = (ctx.input("mask"), outputs.get("height")) {
            let (mask, input) = (mask.grid(), ctx.input_grid("in")?);
            let kept = h.map_indexed(|i, v| if mask.data[i] > 0.0 { v } else { input.data[i] });
            outputs.insert("height".into(), Value::Heightfield(Arc::new(kept)));
        }
        Ok(outputs)
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let d = Domain::new(ctx)?;
        ctx.report_progress(0.0);
        let sim_spec = simulation_spec(ctx.spec, ctx.f64("detail_m"));
        let same = sim_spec == ctx.spec;
        // Inputs on the simulation grid.
        let (terrain, strength, softness): (Grid, Vec<f32>, Vec<f32>) = if same {
            (
                d.terrain.clone(),
                (0..d.len).map(|i| d.strength(i)).collect(),
                (0..d.len).map(|i| d.softness(i)).collect(),
            )
        } else {
            let mask_on_sim = |g: Option<&Grid>, default: f32| -> Vec<f32> {
                match g {
                    Some(g) => resample(g, sim_spec).data.iter().map(|v| snap01(*v)).collect(),
                    None => vec![default; sim_spec.len()],
                }
            };
            let hardness = mask_on_sim(d.hardness, 0.0);
            (
                resample(d.terrain, sim_spec),
                mask_on_sim(d.mask, 1.0),
                hardness.iter().map(|h| 1.0 - h).collect(),
            )
        };
        let start: Vec<f64> = terrain.data.iter().map(|&v| v as f64).collect();
        let sim = simulate(ctx, sim_spec, start.clone(), &strength, &softness)?;
        check_cancel(ctx)?;

        // Masks on the simulation grid.
        let rain = ctx.f64("rainfall_m_yr");
        let depth = |v: f64, half: f64| (v.max(0.0) / (half + v.max(0.0))) as f32;
        let masks: [(&str, Vec<f32>); 4] = [
            (
                "flow",
                sim.discharge
                    .iter()
                    .map(|&v| {
                        if rain > 0.0 {
                            log_mask(v / rain, FLOW_LO_M2, FLOW_HI_M2)
                        } else {
                            0.0
                        }
                    })
                    .collect(),
            ),
            // Net change: where the ground ended lower (worn) or higher
            // (sediment), which is what texturing needs. Material that
            // settled and was carried off again doesn't count.
            (
                "wear",
                start
                    .iter()
                    .zip(&sim.height)
                    .map(|(a, b)| depth(a - b, WEAR_HALF_M))
                    .collect(),
            ),
            (
                "deposition",
                sim.height
                    .iter()
                    .zip(&start)
                    .map(|(a, b)| depth(a - b, DEPOSITION_HALF_M))
                    .collect(),
            ),
            (
                "sediment",
                sim.load.iter().map(|&v| log_mask(v, SED_LO, SED_HI)).collect(),
            ),
        ];

        let mut outputs = if same {
            height_out(sim.height.iter().map(|&v| v as f32).collect(), ctx)
        } else {
            // Add the change (not the coarse surface) to the full-resolution
            // input, so detail smaller than the simulation cells survives.
            let delta = Grid {
                spec: sim_spec,
                data: sim
                    .height
                    .iter()
                    .zip(&start)
                    .map(|(a, b)| (a - b) as f32)
                    .collect(),
            };
            let height = Grid::from_fn_indexed(ctx.spec, |i, x, y| {
                let input = d.terrain.data[i];
                if d.strength(i) > 0.0 {
                    input + delta.sample_bilinear_m(x, y)
                } else {
                    input
                }
            });
            height_out(height.data, ctx)
        };
        for (key, data) in masks {
            let mut grid = Grid { spec: sim_spec, data };
            // Rivers are one simulation cell wide: give the line masks a
            // width of about two cells so they resample cleanly and read as
            // rivers when used for texturing.
            if key == "flow" || key == "sediment" {
                grid = terrain_core::ops::gaussian_blur(&grid, sim_spec.cell_size_m()[0]);
            }
            let grid = if same {
                grid
            } else {
                Grid::from_fn(ctx.spec, |x, y| grid.sample_bilinear_m(x, y).clamp(0.0, 1.0))
            };
            outputs.insert(key.into(), Value::Mask(Arc::new(grid)));
        }
        ctx.report_progress(1.0);
        Ok(outputs)
    }
}
