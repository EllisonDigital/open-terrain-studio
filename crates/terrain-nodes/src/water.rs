//! Water and hydrology nodes: flow, lakes, rivers, sea and snow.
//! See `docs/water.md` for the methods, units and limits.

use std::sync::Arc;

use rayon::prelude::*;
use terrain_core::error::{CoreError, Result};
use terrain_core::ops::{distance_to, gaussian_blur, smoothstep};
use terrain_core::{
    EvalContext, Grid, GridSpec, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType, Value,
};

use crate::hydro::{
    self, FLOW_HI_M2, FLOW_LO_M2, LINK_OUTLET, Routing, flat_jitter, log_mask, resample, route,
    sample_nearest_m, simulation_spec,
};

fn fail(ctx: &EvalContext, message: &str) -> CoreError {
    CoreError::NodeFailed {
        node: ctx.node_id.into(),
        message: message.into(),
    }
}

fn check_cancel(ctx: &EvalContext) -> Result<()> {
    if ctx.is_cancelled() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

/// The terrain input, checked to be finite.
fn terrain<'a>(ctx: &'a EvalContext) -> Result<&'a Grid> {
    let g = ctx.input_grid("in")?.as_ref();
    if g.spec != ctx.spec || g.data.par_iter().any(|v| !v.is_finite() || v.abs() > 1.0e8) {
        return Err(fail(
            ctx,
            "the terrain must be finite and match the evaluation grid",
        ));
    }
    Ok(g)
}

/// "Detail size": the fixed grid drainage is computed on.
fn detail_param(default: f64) -> ParamDef {
    ParamDef::metres("detail_m", "Detail size", default, 1.0, 1000.0).describe(
        "Cell size water is routed at, in metres: about the width of the smallest streams. Smaller is \
         slower. The result is the same at any preview or build resolution.",
    )
}

/// The terrain on the drainage grid for `detail_m`, as f64 heights.
struct Drainage {
    spec: GridSpec,
    same: bool,
    heights: Vec<f64>,
}

impl Drainage {
    fn new(ctx: &EvalContext, terrain: &Grid) -> Self {
        let spec = simulation_spec(ctx.spec, ctx.f64("detail_m"));
        let same = spec == ctx.spec;
        let grid = if same {
            terrain.clone()
        } else {
            resample(terrain, spec)
        };
        Self {
            spec,
            same,
            heights: grid.data.iter().map(|&v| v as f64).collect(),
        }
    }

    fn size(&self) -> (usize, usize) {
        (self.spec.width as usize, self.spec.height as usize)
    }

    fn route(&self) -> Routing {
        let (w, h) = self.size();
        let [dx, dy] = self.spec.cell_size_m();
        route(&self.heights, &flat_jitter(self.heights.len()), w, h, dx, dy)
    }

    /// Back to the evaluation grid: bilinear for smooth values, nearest for
    /// categories and angles.
    fn to_output(&self, ctx: &EvalContext, data: Vec<f32>, smooth: bool) -> Grid {
        let grid = Grid {
            spec: self.spec,
            data,
        };
        if self.same {
            return grid;
        }
        if smooth {
            Grid::from_fn(ctx.spec, |x, y| grid.sample_bilinear_m(x, y))
        } else {
            Grid::from_fn(ctx.spec, |x, y| sample_nearest_m(&grid, x, y))
        }
    }
}

/// Largest value in each cell's 3×3 neighbourhood.
fn dilate(grid: &Grid) -> Grid {
    let (w, h) = (grid.spec.width as i64, grid.spec.height as i64);
    grid.map_indexed(|i, _| {
        let (x, y) = ((i as i64) % w, (i as i64) / w);
        let mut v = f32::MIN;
        for oy in -1..=1 {
            for ox in -1..=1 {
                if (0..w).contains(&(x + ox)) && (0..h).contains(&(y + oy)) {
                    v = v.max(grid.get_clamped(x + ox, y + oy));
                }
            }
        }
        v
    })
}

fn mask(grid: Grid) -> Value {
    Value::Mask(Arc::new(grid.map(|v| v.clamp(0.0, 1.0))))
}

/// White at the waterline, fading to black `width_m` away from it on both
/// sides (in the water and on land).
fn waterline_band(spec: GridSpec, water: &[bool], width_m: f64) -> Grid {
    if !water.contains(&true) {
        return Grid::filled(spec, 0.0);
    }
    let land: Vec<bool> = water.iter().map(|w| !w).collect();
    let to_water = distance_to(spec, water);
    let to_land = distance_to(spec, &land);
    let width = width_m.max(1.0e-3) as f32;
    Grid::from_fn_indexed(spec, |i, _, _| {
        let d = if water[i] {
            to_land.data[i]
        } else {
            to_water.data[i]
        };
        1.0 - smoothstep(0.0, width, d)
    })
}

/// A Heightfield output.
fn heightfield(grid: Grid) -> Value {
    Value::Heightfield(Arc::new(grid))
}

// ---- Lakes ------------------------------------------------------------------

/// Fill the terrain's depressions with water up to where each would spill.
pub struct Lakes {
    schema: NodeSchema,
}

impl Default for Lakes {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "simulate.lakes".into(),
                type_version: 1,
                label: "Lakes".into(),
                category: "Simulate".into(),
                description:
                    "Fills hollows with water up to the level where each would overflow. Height is the \
                              lake bed (flattened by sediment), Water surface the lake level (the ground \
                              elsewhere), Lakes is white on water and Shore along the waterline."
                        .into(),
                inputs: vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
                outputs: vec![
                    PortDef::new("height", "Height", PortType::Heightfield),
                    PortDef::new("water_surface", "Water surface", PortType::Heightfield),
                    PortDef::new("lakes", "Lakes", PortType::Mask),
                    PortDef::new("shore", "Shore", PortType::Mask),
                ],
                params: vec![
                    ParamDef::metres("min_depth_m", "Min depth", 1.0, 0.0, 1000.0)
                        .describe("Hollows shallower than this at their deepest point stay dry."),
                    ParamDef::float("min_area_m2", "Min area", 5000.0, 0.0, 1.0e9)
                        .unit("m²")
                        .describe("Lakes smaller than this stay dry, so puddles don't clutter the map."),
                    ParamDef::float("infill", "Sediment infill", 0.3, 0.0, 1.0).describe(
                        "Share of each lake's depth filled with sediment, giving it a flat floor. \
                         0 keeps the original bed; 1 fills the lake to its surface.",
                    ),
                    ParamDef::metres("shore_width_m", "Shore width", 30.0, 0.0, 10_000.0)
                        .describe("Width of the Shore mask on each side of the waterline, in metres."),
                    detail_param(8.0),
                ],
                gpu: false,
            },
        }
    }
}

/// One lake on the drainage grid.
struct Lake {
    level: f64,
    depth: f64,
}

/// Lakes on the drainage grid: the lake id of every cell (or `usize::MAX`),
/// grown by one cell so the waterline can be found at full resolution.
fn find_lakes(
    heights: &[f64],
    filled: &[f64],
    w: usize,
    ht: usize,
    cell_area: f64,
    min_depth: f64,
    min_area: f64,
) -> (Vec<usize>, Vec<Lake>) {
    const NONE: usize = usize::MAX;
    let n = heights.len();
    let flooded = |i: usize| filled[i] > heights[i];
    let mut id = vec![NONE; n];
    let mut lakes = Vec::new();
    let mut queue = Vec::new();
    // Connected flooded cells at the same level form one lake (8-neighbour),
    // found in index order so ids are deterministic.
    for start in 0..n {
        if id[start] != NONE || !flooded(start) {
            continue;
        }
        let this = lakes.len();
        let level = filled[start];
        let (mut count, mut depth) = (0usize, 0.0f64);
        id[start] = this;
        queue.push(start);
        while let Some(c) = queue.pop() {
            count += 1;
            depth = depth.max(level - heights[c]);
            let (cx, cy) = ((c % w) as i64, (c / w) as i64);
            for (ox, oy) in hydro::D8 {
                let (x, y) = (cx + ox, cy + oy);
                if x < 0 || y < 0 || x >= w as i64 || y >= ht as i64 {
                    continue;
                }
                let j = y as usize * w + x as usize;
                if id[j] == NONE && flooded(j) && filled[j] == level {
                    id[j] = this;
                    queue.push(j);
                }
            }
        }
        lakes.push(Lake { level, depth });
        if depth < min_depth || count as f64 * cell_area < min_area {
            lakes[this].depth = -1.0; // Too small: dropped below.
        }
    }
    for v in id.iter_mut() {
        if *v != NONE && lakes[*v].depth < 0.0 {
            *v = NONE;
        }
    }
    // Grow by one cell (towards the higher lake where two meet).
    let grown: Vec<usize> = (0..n)
        .into_par_iter()
        .map(|i| {
            if id[i] != NONE {
                return id[i];
            }
            let (cx, cy) = ((i % w) as i64, (i / w) as i64);
            let mut best = NONE;
            for (ox, oy) in hydro::D8 {
                let (x, y) = (cx + ox, cy + oy);
                if x < 0 || y < 0 || x >= w as i64 || y >= ht as i64 {
                    continue;
                }
                let j = id[y as usize * w + x as usize];
                if j != NONE && (best == NONE || lakes[j].level > lakes[best].level) {
                    best = j;
                }
            }
            best
        })
        .collect();
    (grown, lakes)
}

impl NodeKind for Lakes {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let t = terrain(ctx)?;
        ctx.report_progress(0.0);
        let d = Drainage::new(ctx, t);
        let (w, ht) = d.size();
        let [dx, dy] = d.spec.cell_size_m();
        let filled = hydro::fill_depressions(&d.heights, w, ht);
        check_cancel(ctx)?;
        ctx.report_progress(0.5);
        let (ids, lakes) = find_lakes(
            &d.heights,
            &filled,
            w,
            ht,
            dx * dy,
            ctx.f64("min_depth_m"),
            ctx.f64("min_area_m2"),
        );
        check_cancel(ctx)?;

        // Lake id at full resolution: nearest drainage cell.
        let id_grid = Grid {
            spec: d.spec,
            data: ids
                .iter()
                .map(|&i| if i == usize::MAX { -1.0 } else { i as f32 })
                .collect(),
        };
        let infill = ctx.f64("infill");
        let n = ctx.spec.len();
        let mut bed = vec![0.0f32; n];
        let mut surface = vec![0.0f32; n];
        let mut water = vec![false; n];
        let mut lake_mask = vec![0.0f32; n];
        let spec = ctx.spec;
        let rows = bed
            .par_chunks_mut(spec.width as usize)
            .zip(surface.par_chunks_mut(spec.width as usize))
            .zip(water.par_chunks_mut(spec.width as usize))
            .zip(lake_mask.par_chunks_mut(spec.width as usize));
        rows.enumerate().for_each(|(j, (((bed, surface), water), mask))| {
            let y = spec.y_m(j as u32);
            for i in 0..bed.len() {
                let h = t.data[j * spec.width as usize + i] as f64;
                let x = spec.x_m(i as u32);
                let id = sample_nearest_m(&id_grid, x, y);
                let (mut b, mut s, mut m) = (h, h, 0.0);
                if id >= 0.0 {
                    let lake = &lakes[id as usize];
                    if lake.level > h {
                        b = h.max(lake.level - (1.0 - infill) * lake.depth);
                        s = lake.level;
                        // Soft only over the last 10 cm, so shallow edges stay water.
                        m = ((lake.level - h) / 0.1).min(1.0);
                    }
                }
                bed[i] = b as f32;
                surface[i] = s as f32;
                water[i] = m >= 0.5;
                mask[i] = m as f32;
            }
        });
        let shore = waterline_band(spec, &water, ctx.f64("shore_width_m"));
        ctx.report_progress(1.0);
        Ok(Outputs::from([
            ("height".into(), heightfield(Grid { spec, data: bed })),
            ("water_surface".into(), heightfield(Grid { spec, data: surface })),
            (
                "lakes".into(),
                mask(Grid {
                    spec,
                    data: lake_mask,
                }),
            ),
            ("shore".into(), mask(shore)),
        ]))
    }
}

// ---- Rivers -----------------------------------------------------------------

/// Carve river channels where enough water collects.
pub struct Rivers {
    schema: NodeSchema,
}

impl Default for Rivers {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "simulate.rivers".into(),
                type_version: 1,
                label: "Rivers".into(),
                category: "Simulate".into(),
                description: "Carves river channels where the water draining through a point exceeds Source \
                              area, widening and deepening downstream. Rivers run to the world's edges, \
                              crossing hollows at their spill level. Outputs the carved Height, the Water \
                              surface, the River mask and a Riverbank mask."
                    .into(),
                inputs: vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
                outputs: vec![
                    PortDef::new("height", "Height", PortType::Heightfield),
                    PortDef::new("water_surface", "Water surface", PortType::Heightfield),
                    PortDef::new("river", "River", PortType::Mask),
                    PortDef::new("riverbank", "Riverbank", PortType::Mask),
                ],
                params: vec![
                    ParamDef::float("source_area_km2", "Source area", 0.5, 0.001, 10_000.0)
                        .unit("km²")
                        .describe(
                            "Area that must drain through a point before a river starts there. Smaller \
                             values give more, smaller streams.",
                        ),
                    ParamDef::metres("width_m", "Width", 40.0, 0.1, 5000.0).describe(
                        "Width of the largest rivers (draining 1000 × Source area), in metres. Streams \
                         start at a tenth of this.",
                    ),
                    ParamDef::metres("depth_m", "Depth", 4.0, 0.0, 500.0)
                        .describe("Depth of the largest rivers below their water surface, in metres."),
                    ParamDef::metres("bank_width_m", "Bank width", 20.0, 0.0, 5000.0)
                        .describe("Width of the sloping bank beside each river, and of the Riverbank mask."),
                    detail_param(8.0),
                ],
                gpu: false,
            },
        }
    }
}

/// A short straight piece of river centreline with its properties at each
/// end: water level (m), half width (m) and depth (m).
#[derive(Clone, Copy)]
struct Reach {
    a: [f64; 2],
    b: [f64; 2],
    level: [f64; 2],
    half_width: [f64; 2],
    depth: [f64; 2],
}

impl Reach {
    /// Reach of influence beyond the centreline, metres.
    fn radius(&self, bank: f64) -> f64 {
        self.half_width[0].max(self.half_width[1]) + bank
    }
}

/// Each river bend (a quadratic Bézier) is drawn as this many straight reaches.
const PIECES: usize = 4;

/// River centrelines from the routing: every river cell draws a curve from
/// the middle of its link to the middle of its receiver's link, so D8 steps
/// become smooth bends and tributaries join smoothly.
fn river_reaches(d: &Drainage, r: &Routing, area: &[f64], ctx: &EvalContext) -> Vec<Reach> {
    let (w, _) = d.size();
    let source = ctx.f64("source_area_km2") * 1.0e6;
    let (max_w, depth) = (ctx.f64("width_m"), ctx.f64("depth_m"));
    let pos = |i: usize| [d.spec.x_m((i % w) as u32), d.spec.y_m((i / w) as u32)];
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    // Size grows with drained area on a log scale, from a tenth at the source
    // to full size at 1000 × the source area.
    let size = |i: usize| log_mask(area[i], source, source * 1000.0) as f64;
    let half_width = |i: usize| 0.5 * max_w * (0.1 + 0.9 * size(i));
    let deep = |i: usize| depth * (0.3 + 0.7 * size(i));
    let level = |i: usize| r.filled[i];
    let is_river = |i: usize| area[i] >= source;
    let mut reaches = Vec::new();
    // Downstream-first order keeps the list deterministic.
    for &i in &r.stack {
        let j = r.receiver[i];
        if j == i || !is_river(i) {
            continue;
        }
        let k = r.receiver[j];
        let (pi, pj) = (pos(i), pos(j));
        let start = mid(pi, pj);
        let (end, end_level, end_hw, end_depth) = if k == j {
            (pj, level(j), half_width(j), deep(j))
        } else {
            (
                mid(pj, pos(k)),
                0.5 * (level(j) + level(k)),
                half_width(j),
                deep(j),
            )
        };
        let start_level = 0.5 * (level(i) + level(j));
        let (start_hw, start_depth) = (0.5 * (half_width(i) + half_width(j)), 0.5 * (deep(i) + deep(j)));
        let point = |t: f64| {
            let u = 1.0 - t;
            [
                u * u * start[0] + 2.0 * u * t * pj[0] + t * t * end[0],
                u * u * start[1] + 2.0 * u * t * pj[1] + t * t * end[1],
            ]
        };
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        for p in 0..PIECES {
            let (t0, t1) = (p as f64 / PIECES as f64, (p + 1) as f64 / PIECES as f64);
            reaches.push(Reach {
                a: point(t0),
                b: point(t1),
                level: [lerp(start_level, end_level, t0), lerp(start_level, end_level, t1)],
                half_width: [lerp(start_hw, end_hw, t0), lerp(start_hw, end_hw, t1)],
                depth: [lerp(start_depth, end_depth, t0), lerp(start_depth, end_depth, t1)],
            });
        }
    }
    // Headwaters: from each river cell no river flows into, to the middle
    // of its first link.
    let mut fed = vec![false; area.len()];
    for &i in &r.stack {
        if is_river(i) && r.receiver[i] != i {
            fed[r.receiver[i]] = true;
        }
    }
    for &i in &r.stack {
        let j = r.receiver[i];
        if j == i || !is_river(i) || fed[i] {
            continue;
        }
        let pi = pos(i);
        reaches.push(Reach {
            a: pi,
            b: mid(pi, pos(j)),
            level: [level(i), 0.5 * (level(i) + level(j))],
            half_width: [half_width(i), 0.5 * (half_width(i) + half_width(j))],
            depth: [deep(i), 0.5 * (deep(i) + deep(j))],
        });
    }
    reaches
}

impl NodeKind for Rivers {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let t = terrain(ctx)?;
        ctx.report_progress(0.0);
        let d = Drainage::new(ctx, t);
        let [dx, dy] = d.spec.cell_size_m();
        let r = d.route();
        check_cancel(ctx)?;
        let area = hydro::accumulate_d8(&r, dx * dy);
        let reaches = river_reaches(&d, &r, &area, ctx);
        check_cancel(ctx)?;
        ctx.report_progress(0.4);

        let spec = ctx.spec;
        let (w, ht) = (spec.width as usize, spec.height as usize);
        let bank = ctx.f64("bank_width_m");
        let [cx, cy] = spec.cell_size_m();
        // Bucket reaches by blocks of rows; each block is then independent.
        const BLOCK: usize = 32;
        let blocks = ht.div_ceil(BLOCK);
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); blocks];
        let row_of = |y: f64| (y - spec.origin_m[1]) / cy;
        for (n, reach) in reaches.iter().enumerate() {
            let rad = reach.radius(bank);
            let lo = row_of(reach.a[1].min(reach.b[1]) - rad).floor().max(0.0) as usize;
            let hi = (row_of(reach.a[1].max(reach.b[1]) + rad).ceil().max(0.0) as usize).min(ht - 1);
            if lo > hi {
                continue;
            }
            for bucket in &mut buckets[lo / BLOCK..=hi / BLOCK] {
                bucket.push(n as u32);
            }
        }

        let mut height = t.data.clone();
        let mut surface = t.data.clone();
        let mut river = vec![0.0f32; spec.len()];
        let mut riverbank = vec![0.0f32; spec.len()];
        height
            .par_chunks_mut(w * BLOCK)
            .zip(surface.par_chunks_mut(w * BLOCK))
            .zip(river.par_chunks_mut(w * BLOCK))
            .zip(riverbank.par_chunks_mut(w * BLOCK))
            .enumerate()
            .for_each(|(block, (((height, surface), river), riverbank))| {
                let j0 = block * BLOCK;
                let rows = height.len() / w;
                let mut water_level = vec![f64::NEG_INFINITY; height.len()];
                for &n in &buckets[block] {
                    let reach = &reaches[n as usize];
                    let rad = reach.radius(bank);
                    let (x0, x1) = (reach.a[0].min(reach.b[0]) - rad, reach.a[0].max(reach.b[0]) + rad);
                    let (y0, y1) = (reach.a[1].min(reach.b[1]) - rad, reach.a[1].max(reach.b[1]) + rad);
                    let i_lo = ((x0 - spec.origin_m[0]) / cx).floor().max(0.0) as usize;
                    let i_hi = (((x1 - spec.origin_m[0]) / cx).ceil().max(0.0) as usize).min(w - 1);
                    let r_lo = (row_of(y0).floor().max(j0 as f64) as usize).max(j0);
                    let r_hi = (row_of(y1).ceil().max(0.0) as usize).min(j0 + rows - 1);
                    let (ax, ay) = (reach.b[0] - reach.a[0], reach.b[1] - reach.a[1]);
                    let len2 = ax * ax + ay * ay;
                    for j in r_lo..=r_hi {
                        let y = spec.y_m(j as u32);
                        for i in i_lo..=i_hi {
                            let x = spec.x_m(i as u32);
                            let (px, py) = (x - reach.a[0], y - reach.a[1]);
                            let s = if len2 > 0.0 {
                                ((px * ax + py * ay) / len2).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let (qx, qy) = (px - s * ax, py - s * ay);
                            let dist = (qx * qx + qy * qy).sqrt();
                            let hw = reach.half_width[0] + (reach.half_width[1] - reach.half_width[0]) * s;
                            if dist >= hw + bank {
                                continue;
                            }
                            let level = reach.level[0] + (reach.level[1] - reach.level[0]) * s;
                            let depth = reach.depth[0] + (reach.depth[1] - reach.depth[0]) * s;
                            let k = (j - j0) * w + i;
                            let h = t.data[j * w + i] as f64;
                            let target = if dist < hw {
                                let u = dist / hw;
                                level - depth * (1.0 - u * u)
                            } else if bank > 0.0 {
                                let u = ((dist - hw) / bank) as f32;
                                level + smoothstep(0.0, 1.0, u) as f64 * (h - level)
                            } else {
                                h
                            };
                            height[k] = height[k].min(target.min(h) as f32);
                            if dist < hw {
                                // Soft over the last metre of the bank edge.
                                river[k] = river[k].max(((hw - dist) as f32 + 0.5).clamp(0.0, 1.0));
                                water_level[k] = water_level[k].max(level);
                            } else if bank > 0.0 {
                                let u = ((dist - hw) / bank) as f32;
                                riverbank[k] = riverbank[k].max(1.0 - smoothstep(0.0, 1.0, u));
                            }
                        }
                    }
                }
                for k in 0..height.len() {
                    riverbank[k] *= 1.0 - river[k];
                    surface[k] = if water_level[k] > height[k] as f64 {
                        water_level[k] as f32
                    } else {
                        height[k]
                    };
                }
            });
        ctx.report_progress(1.0);
        Ok(Outputs::from([
            ("height".into(), heightfield(Grid { spec, data: height })),
            ("water_surface".into(), heightfield(Grid { spec, data: surface })),
            ("river".into(), mask(Grid { spec, data: river })),
            (
                "riverbank".into(),
                mask(Grid {
                    spec,
                    data: riverbank,
                }),
            ),
        ]))
    }
}

// ---- Flow -------------------------------------------------------------------

/// Tarboton's D-infinity facets: (cardinal neighbour, diagonal neighbour).
const FACETS: [((i64, i64), (i64, i64)); 8] = [
    ((1, 0), (1, -1)),
    ((0, -1), (1, -1)),
    ((0, -1), (-1, -1)),
    ((-1, 0), (-1, -1)),
    ((-1, 0), (-1, 1)),
    ((0, 1), (-1, 1)),
    ((0, 1), (1, 1)),
    ((1, 0), (1, 1)),
];

/// Where D-infinity sends a cell's water: up to two neighbours with shares,
/// and the flow direction in degrees (0° = +X, 90° = +Y).
struct Split {
    to: [usize; 2],
    share: [f64; 2],
    angle_deg: f64,
}

/// D-infinity (Tarboton 1997) on the filled surface `z`, for an interior cell.
fn dinf(z: &[f64], i: usize, w: usize, dx: f64, dy: f64) -> Option<Split> {
    let (cx, cy) = ((i % w) as i64, (i / w) as i64);
    let at = |(ox, oy): (i64, i64)| (cy + oy) as usize * w + (cx + ox) as usize;
    let mut best: Option<(f64, Split)> = None;
    for (card, diag) in FACETS {
        let along_x = card.0 != 0;
        let (d1, d2) = if along_x { (dx, dy) } else { (dy, dx) };
        let (e0, e1, e2) = (z[i], z[at(card)], z[at(diag)]);
        let s1 = (e0 - e1) / d1;
        let s2 = (e1 - e2) / d2;
        let r_max = libm::atan2(d2, d1);
        let (r, s) = {
            let r = libm::atan2(s2, s1);
            if r < 0.0 {
                (0.0, s1)
            } else if r > r_max {
                (r_max, (e0 - e2) / (d1 * d1 + d2 * d2).sqrt())
            } else {
                (r, (s1 * s1 + s2 * s2).sqrt())
            }
        };
        if s <= 0.0 || best.as_ref().is_some_and(|(b, _)| s <= *b) {
            continue;
        }
        let to_diag = r / r_max;
        // Direction: along the cardinal step, turned by r towards the diagonal.
        let card_angle = libm::atan2(card.1 as f64, card.0 as f64);
        let perp = (diag.0 - card.0, diag.1 - card.1);
        let perp_angle = libm::atan2(perp.1 as f64, perp.0 as f64);
        let (vx, vy) = (
            libm::cos(card_angle) * libm::cos(r) + libm::cos(perp_angle) * libm::sin(r),
            libm::sin(card_angle) * libm::cos(r) + libm::sin(perp_angle) * libm::sin(r),
        );
        best = Some((
            s,
            Split {
                to: [at(card), at(diag)],
                share: [1.0 - to_diag, to_diag],
                angle_deg: degrees(vx, vy),
            },
        ));
    }
    best.map(|(_, split)| split)
}

/// Angle of (x, y) in degrees, 0..360.
fn degrees(x: f64, y: f64) -> f64 {
    let a = libm::atan2(y, x).to_degrees();
    if a < 0.0 { a + 360.0 } else { a }
}

/// Drained area (m²) and flow direction (degrees) of every cell.
fn accumulate(
    r: &Routing,
    z: &[f64],
    w: usize,
    ht: usize,
    dx: f64,
    dy: f64,
    d_inf: bool,
) -> (Vec<f64>, Vec<f64>) {
    let n = z.len();
    let d8_angle = |i: usize| -> f64 {
        let j = r.receiver[i];
        if j == i {
            // Outlets drain off the nearest edge.
            let (x, y) = (i % w, i / w);
            let edges = [(x, 180.0), (w - 1 - x, 0.0), (y, 270.0), (ht - 1 - y, 90.0)];
            return edges.iter().min_by_key(|e| e.0).unwrap().1;
        }
        let (ox, oy) = ((j % w) as f64 - (i % w) as f64, (j / w) as f64 - (i / w) as f64);
        degrees(ox * dx, oy * dy)
    };
    if !d_inf {
        let angles = (0..n).into_par_iter().map(d8_angle).collect();
        return (hydro::accumulate_d8(r, dx * dy), angles);
    }
    let splits: Vec<Option<Split>> = (0..n)
        .into_par_iter()
        .map(|i| {
            if r.link[i] == LINK_OUTLET {
                None
            } else {
                dinf(z, i, w, dx, dy)
            }
        })
        .collect();
    // Receivers are strictly lower on the filled surface, so they come
    // earlier in the routing stack: walk it backwards.
    let mut area = vec![dx * dy; n];
    for &i in r.stack.iter().rev() {
        match &splits[i] {
            Some(s) => {
                let a = area[i];
                for k in 0..2 {
                    if s.share[k] > 0.0 {
                        area[s.to[k]] += a * s.share[k];
                    }
                }
            }
            None => {
                let j = r.receiver[i];
                if j != i {
                    let a = area[i];
                    area[j] += a;
                }
            }
        }
    }
    let angles = (0..n)
        .into_par_iter()
        .map(|i| splits[i].as_ref().map_or_else(|| d8_angle(i), |s| s.angle_deg))
        .collect();
    (area, angles)
}

/// Flow accumulation, flow direction and drainage basins.
pub struct Flow {
    schema: NodeSchema,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            schema: NodeSchema {
                type_id: "data.flow".into(),
                type_version: 1,
                label: "Flow".into(),
                category: "Data".into(),
                description: "Where rain would run: Flow is bright along streams and rivers (the area \
                              draining through each point, on a log scale), Direction is the way water \
                              flows (0..1 = 0..360°, 0° = +X), and Basins gives each drainage basin its own \
                              value. Pits fill until they spill; water leaves at the world's edges."
                    .into(),
                inputs: vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
                outputs: vec![
                    PortDef::new("accumulation", "Flow", PortType::Mask),
                    PortDef::new("direction", "Direction", PortType::Mask),
                    PortDef::new("basins", "Basins", PortType::Mask),
                ],
                params: vec![
                    ParamDef::choice(
                        "method",
                        "Method",
                        "dinf",
                        &[("dinf", "D-infinity (smooth)"), ("d8", "D8 (single path)")],
                    )
                    .describe(
                        "D-infinity splits water between two downhill neighbours, so flow spreads on open \
                         slopes; D8 sends it all one way, giving sharp single-cell streams.",
                    ),
                    detail_param(8.0),
                ],
                gpu: false,
            },
        }
    }
}

impl NodeKind for Flow {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let t = terrain(ctx)?;
        ctx.report_progress(0.0);
        let d = Drainage::new(ctx, t);
        let (w, ht) = d.size();
        let [dx, dy] = d.spec.cell_size_m();
        let r = d.route();
        check_cancel(ctx)?;
        ctx.report_progress(0.5);
        let (area, angle) = accumulate(&r, &r.filled, w, ht, dx, dy, ctx.choice("method") == "dinf");
        check_cancel(ctx)?;

        // Basins: every cell takes its outlet's (fixed, pseudo-random) value.
        let mut basin = vec![0usize; area.len()];
        for &i in &r.stack {
            let j = r.receiver[i];
            basin[i] = if j == i { i } else { basin[j] };
        }
        let flow = Grid {
            spec: d.spec,
            data: area
                .iter()
                .map(|&a| log_mask(a, FLOW_LO_M2, FLOW_HI_M2))
                .collect(),
        };
        // Streams are one drainage cell wide: widen them to about three,
        // keeping their brightness, so they resample cleanly.
        let flow = gaussian_blur(&dilate(&flow), 0.5 * d.spec.cell_size_m()[0]);
        let outputs = Outputs::from([
            ("accumulation".into(), mask(d.to_output(ctx, flow.data, true))),
            (
                "direction".into(),
                mask(d.to_output(ctx, angle.iter().map(|&a| (a / 360.0) as f32).collect(), false)),
            ),
            (
                "basins".into(),
                mask(
                    d.to_output(
                        ctx,
                        basin
                            .iter()
                            .map(|&o| hydro::unit_hash(o as u64 ^ 0xba51_5eed) as f32)
                            .collect(),
                        false,
                    ),
                ),
            ),
        ]);
        ctx.report_progress(1.0);
        Ok(outputs)
    }
}
