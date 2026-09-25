use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
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
///    (Braun & Willett 2013), so any time step is stable.
/// 4. Sediment is carried downstream; where it exceeds the flow's capacity
///    (proportional to its stream power) the excess settles, filling lakes and
///    flats first.
/// 5. Hillslope creep: linear diffusion, applied exactly as a Gaussian blur
///    with σ = √(2·D·dt) metres, so it is the same at any resolution.
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
/// Routing gradient added across flats and filled pits (m per m).
const EPSILON_SLOPE: f64 = 1.0e-5;
/// Flow mask: drained area (m²) on a log scale, black to white.
const FLOW_LO_M2: f64 = 5.0e3;
const FLOW_HI_M2: f64 = 5.0e7;
/// Sediment mask: sediment load (m³/yr) on a log scale.
const SED_LO: f64 = 1.0;
const SED_HI: f64 = 1.0e5;
/// Wear and deposition masks: net depth (m) at half brightness.
const WEAR_HALF_M: f64 = 30.0;
const DEPOSITION_HALF_M: f64 = 10.0;

/// Neighbour offsets (dx, dy) in a fixed order: 4 edges, then 4 diagonals.
const D8: [(i64, i64); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

/// An f64 as an order-preserving integer, for the priority queue.
#[inline]
fn order_key(v: f64) -> u64 {
    let b = v.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

/// A fixed pseudo-random value in 0..1 for a cell.
#[inline]
fn unit_hash(i: u64) -> f64 {
    (terrain_core::seed::mix64(i ^ 0x9e37_79b9_7f4a_7c15) >> 11) as f64 / (1u64 << 53) as f64
}

/// Kinds of link from a cell to its receiver, indexing [`Routing::lengths`]
/// and the per-step tables built from them.
const LINK_X: u8 = 0;
const LINK_Y: u8 = 1;
const LINK_DIAGONAL: u8 = 2;
const LINK_OUTLET: u8 = 3;

/// Flow routing for one surface.
struct Routing {
    /// Downstream neighbour of each cell (itself for outlets).
    receiver: Vec<usize>,
    /// Kind of link to the receiver (`LINK_*`).
    link: Vec<u8>,
    /// Length of each kind of link, metres (1 for outlets).
    lengths: [f64; 4],
    /// Cells ordered so every receiver comes before its donors.
    stack: Vec<usize>,
    /// Surface with pits filled (routing only).
    filled: Vec<f64>,
}

impl Routing {
    /// Distance from cell `i` to its receiver, metres.
    #[inline]
    fn distance(&self, i: usize) -> f64 {
        self.lengths[self.link[i] as usize]
    }

    /// `f(length)` for every kind of link, so per-cell work can look it up.
    fn per_link(&self, f: impl Fn(f64) -> f64) -> [f64; 4] {
        self.lengths.map(f)
    }
}

/// The fixed random factor `0.25 + 1.5 × unit_hash(cell)` that makes ε
/// routing wander across flats (the same for every step).
fn flat_jitter(n: usize) -> Vec<f64> {
    (0..n)
        .into_par_iter()
        .map(|j| 0.25 + 1.5 * unit_hash(j as u64))
        .collect()
}

fn route(h: &[f64], jitter: &[f64], w: usize, ht: usize, dx: f64, dy: f64) -> Routing {
    let n = h.len();
    let is_edge = |i: usize| {
        let (x, y) = (i % w, i / w);
        x == 0 || y == 0 || x + 1 == w || y + 1 == ht
    };
    // Distances for D8 neighbours: x, x, y, y, then diagonals.
    let diag = (dx * dx + dy * dy).sqrt();
    let step = [dx, dx, dy, dy, diag, diag, diag, diag];
    // Priority-Flood+ε pops cells in order of (filled height, index): every
    // cell it pushes is above the one just popped, so that order only rises.
    //
    // Most cells end up unraised (filled = own height). Such a cell is always
    // discovered before its turn (by a lower neighbour), so these cells pop
    // exactly in order of (height, index): sort them once, in parallel, and
    // walk that list. Only raised cells (pits and flats) go through a heap.
    // A cell still undiscovered when its turn in the list comes will be
    // raised, and joins the heap when discovered. Merging the two gives the
    // same pop order as one heap of every cell.
    let key = |height: f64, cell: usize| ((order_key(height) as u128) << 64) | cell as u128;
    let mut by_height: Vec<u128> = (0..n).into_par_iter().map(|i| key(h[i], i)).collect();
    by_height.par_sort_unstable();
    let mut filled = vec![f64::NAN; n];
    let mut stack = Vec::with_capacity(n);
    let mut raised: BinaryHeap<Reverse<u128>> = BinaryHeap::new();
    for i in (0..n).filter(|&i| is_edge(i)) {
        filled[i] = h[i];
    }
    let mut next = 0;
    loop {
        let top = raised.peek().map(|r| r.0);
        let c = match by_height.get(next) {
            Some(&k) if top.is_none_or(|t| k < t) => {
                next += 1;
                let c = k as u64 as usize;
                // Undiscovered (will be raised) or raised (in the heap): not now.
                if filled[c] != h[c] {
                    continue;
                }
                c
            }
            _ => match raised.pop() {
                Some(Reverse(k)) => k as u64 as usize,
                None => break,
            },
        };
        stack.push(c);
        let (cx, cy) = ((c % w) as i64, (c / w) as i64);
        for (k, (ox, oy)) in D8.iter().enumerate() {
            let (x, y) = (cx + ox, cy + oy);
            if x < 0 || y < 0 || x >= w as i64 || y >= ht as i64 {
                continue;
            }
            let j = y as usize * w + x as usize;
            if !filled[j].is_nan() {
                continue;
            }
            // Jittered ε: across flats and filled pits a plain ε makes flow
            // run in straight lines; a per-cell random factor lets it wander.
            filled[j] = h[j].max(filled[c] + EPSILON_SLOPE * step[k] * jitter[j]);
            if filled[j] != h[j] {
                raised.push(Reverse(key(filled[j], j)));
            }
        }
    }
    // Steepest descent on the filled surface (always downhill, thanks to ε).
    let kind = [
        LINK_X,
        LINK_X,
        LINK_Y,
        LINK_Y,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
    ];
    let (receiver, link): (Vec<usize>, Vec<u8>) = (0..n)
        .into_par_iter()
        .map(|i| {
            if is_edge(i) {
                return (i, LINK_OUTLET);
            }
            let (cx, cy) = ((i % w) as i64, (i / w) as i64);
            let mut best = (i, LINK_OUTLET, 0.0);
            for (k, (ox, oy)) in D8.iter().enumerate() {
                let j = (cy + oy) as usize * w + (cx + ox) as usize;
                let s = (filled[i] - filled[j]) / step[k];
                if s > best.2 {
                    best = (j, kind[k], s);
                }
            }
            (best.0, best.1)
        })
        .unzip();
    Routing {
        receiver,
        link,
        lengths: [dx, dy, diag, 1.0],
        stack,
        filled,
    }
}

/// Map a positive quantity to 0..1 on a log scale between `lo` and `hi`.
#[inline]
fn log_mask(v: f64, lo: f64, hi: f64) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    (libm::log10(v.max(lo) / lo) / libm::log10(hi / lo)).min(1.0) as f32
}

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

/// The simulation grid: the canonical grid with `detail_m` cells over the
/// region, so every resolution finer than twice the detail size simulates
/// exactly the same rivers. Much coarser grids (quick previews) simulate on
/// themselves.
fn simulation_spec(spec: GridSpec, detail_m: f64) -> GridSpec {
    let cell = spec.cell_size_m();
    if cell[0] >= 2.0 * detail_m && cell[1] >= 2.0 * detail_m {
        return spec;
    }
    let samples =
        |extent: f64| ((extent / detail_m).round() as u32 + 1).clamp(2, terrain_core::grid::MAX_RESOLUTION);
    let canonical = GridSpec {
        width: samples(spec.extent_m[0]),
        height: samples(spec.extent_m[1]),
        ..spec
    };
    if canonical.width == spec.width && canonical.height == spec.height {
        spec
    } else {
        canonical
    }
}

/// Resample onto `to` after a low-pass filter of half a `to` cell (in
/// metres), whatever the source resolution, so every resolution feeds the
/// simulation the same band-limited terrain.
fn resample(grid: &Grid, to: GridSpec) -> Grid {
    let smooth = terrain_core::ops::gaussian_blur(grid, 0.5 * to.cell_size_m()[0]);
    Grid::from_fn(to, |x, y| smooth.sample_bilinear_m(x, y))
}

impl NodeKind for Hydraulic {
    fn schema(&self) -> &NodeSchema {
        &self.schema
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

/// Resampled 0..1 masks: snap values within rounding of 0 or 1, so a
/// uniform "fully hard" or "fully protected" map stays exactly that.
#[inline]
fn snap01(v: f32) -> f32 {
    if v < 1.0e-6 {
        0.0
    } else if v > 1.0 - 1.0e-6 {
        1.0
    } else {
        v
    }
}
