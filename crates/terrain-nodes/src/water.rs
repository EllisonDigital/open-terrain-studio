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
