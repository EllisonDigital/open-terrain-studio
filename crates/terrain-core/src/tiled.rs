//! Tiled evaluation: builds bigger than one grid can hold (ARCHITECTURE.md §4).
//!
//! The build grid is split into tiles. Each tile's *core* is a block of build
//! samples; the cores cover every sample exactly once. Every node is
//! evaluated over its tile's core grown by a margin, so that everything
//! downstream is exact over the core:
//!
//! - A **local** node ([`Reach::Local`]) reads inputs up to its reach away.
//!   Its inputs must be exact over its own region, so each producer's
//!   margin covers the consumer's margin plus the consumer's reach. Tiles
//!   of local graphs are bit-identical to an untiled build.
//! - A **global** node ([`Reach::Global`]: water routing, erosion, auto
//!   ranges) gets a *world pass*: its inputs are streamed tile by tile onto
//!   one whole-world grid of its choosing ([`NodeKind::world_spec`],
//!   filtered like the simulations filter their inputs), it runs once on
//!   that grid, and each tile's result is brought back to build resolution
//!   by [`NodeKind::finish_tile`], keeping the tile's own fine detail.
//!
//! Global nodes are run in waves: first those with no global node upstream,
//! then those that only depend on the first wave, and so on. A final pass
//! evaluates the requested outputs tile by tile and hands each tile's core
//! to a sink, so no whole-world grid at build resolution ever exists.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::cache::{CacheKey, key_of};
use crate::error::{CoreError, Result};
use crate::eval::{Compute, EvalOptions, NodeStep, consumer_counts, release_inputs};
use crate::graph::Graph;
use crate::grid::{Grid, GridSpec};
use crate::node::{NodeRegistry, Outputs, PortType, Reach, Value, WorldPass};
use crate::ops::gaussian_blur;
use crate::world::World;

/// Build resolutions above this are evaluated in tiles.
pub const TILED_ABOVE: u32 = 4097;

/// Default tile core size, in samples per side.
pub const DEFAULT_TILE_SIZE: u32 = 2048;

/// A local node whose reach is more than this fraction of the world is
/// treated as global: tiles reading most of the world gain nothing.
const MAX_LOCAL_FRACTION: f64 = 0.25;

/// One tile of a tiled evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    /// Column and row of the tile among all tiles.
    pub index: [u32; 2],
    /// The samples this tile is responsible for: a window of the build grid.
    pub core: GridSpec,
}

impl Tile {
    /// Whether world position (x, y) lies in this tile's share of the world:
    /// from its first sample up to (not including) the next tile's first
    /// sample, or to the world's far edge for the last tiles.
    pub fn owns(&self, x_m: f64, y_m: f64) -> bool {
        let c = self.core;
        let whole = c.whole();
        let o = c.offset();
        let within = |v: f64, start: u32, len: u32, n: u32, at: &dyn Fn(u32) -> f64| {
            let end = start + len;
            v >= at(start) && if end >= n { v <= at(n - 1) } else { v < at(end) }
        };
        within(x_m, o[0], c.width, whole.width, &|i| whole.x_m(i))
            && within(y_m, o[1], c.height, whole.height, &|j| whole.y_m(j))
    }
}

/// Split `n` samples into runs of `size` (the last run absorbs a remainder
/// under 2 samples, since a grid needs at least 2). Returns `(start, len)`.
fn runs(n: u32, size: u32) -> Vec<(u32, u32)> {
    let size = size.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let len = size.min(n - start);
        out.push((start, len));
        start += len;
    }
    if out.len() > 1 && out.last().is_some_and(|r| r.1 < 2) {
        let (_, extra) = out.pop().expect("checked");
        out.last_mut().expect("checked").1 += extra;
    }
    out
}

/// The tiles of `full` (a whole build grid) with cores of `size` samples.
pub fn tiles(full: GridSpec, size: u32) -> Vec<Tile> {
    let (cols, rows) = (runs(full.width, size), runs(full.height, size));
    let mut out = Vec::with_capacity(cols.len() * rows.len());
    for (tj, &(j0, h)) in rows.iter().enumerate() {
        for (ti, &(i0, w)) in cols.iter().enumerate() {
            out.push(Tile {
                index: [ti as u32, tj as u32],
                core: full.window(i0, j0, w, h),
            });
        }
    }
    out
}

/// How a node takes part in a tiled evaluation.
struct Planned<'g> {
    step: NodeStep<'g>,
    /// A global node, with a world pass.
    global: bool,
    /// How far the node reads around each sample, in build cells: its
    /// reach, or for a global node how far finishing a tile reads.
    reach: u32,
    /// Cells around the core over which this node's result must be exact.
    need: u32,
    /// Global nodes: the world-pass grid (`None`: ranges only).
    world_spec: Option<GridSpec>,
    /// Global nodes: cells of input needed around a core to fill its part of
    /// the world-pass grid.
    halo: u32,
    /// Global nodes: how many waves of global nodes come before this one.
    wave: usize,
}

impl Planned<'_> {
    /// Cells around the core this node is evaluated over.
    fn margin(&self) -> u32 {
        self.need + self.reach
    }
}

/// A plan for evaluating some node outputs over a build grid in tiles.
pub struct TiledPlan<'g> {
    graph: &'g Graph,
    world: &'g World,
    full: GridSpec,
    order: Vec<String>,
    nodes: BTreeMap<String, Planned<'g>>,
    waves: Vec<Vec<String>>,
    tiles: Vec<Tile>,
}

impl<'g> TiledPlan<'g> {
    /// Plan the evaluation of `targets` (node ids) over `full`, with tile
    /// cores of `tile_size` samples.
    pub fn new(
        graph: &'g Graph,
        registry: &'g NodeRegistry,
        world: &'g World,
        full: GridSpec,
        targets: &[&str],
        tile_size: u32,
    ) -> Result<Self> {
        let full = full.whole();
        let order = graph.evaluation_order_of(targets)?;
        let cell = full.cell_size_m();
        let cell = cell[0].min(cell[1]);
        let limit = MAX_LOCAL_FRACTION * full.extent_m[0].max(full.extent_m[1]);
        let mut nodes: BTreeMap<String, Planned<'g>> = BTreeMap::new();
        for id in &order {
            let id: &'g str = graph.node(id).map(|n| n.id.as_str()).expect("in the order");
            let step = NodeStep::new(graph, registry, world, id)?;
            let ctx = step.bare_context(world, full);
            let cells = |r: f64| if r > 0.0 { (r / cell).ceil() as u32 + 1 } else { 0 };
            let local = match step.kind.reach(&ctx) {
                Reach::Local(r) if r.is_finite() && r <= limit => Some(cells(r.max(0.0))),
                _ => None,
            };
            let global = local.is_none();
            let reach = local.unwrap_or_else(|| cells(step.kind.finish_reach(&ctx).min(limit)));
            let (world_spec, halo) = match local {
                Some(_) => (None, 0),
                None => {
                    let spec = step.kind.world_spec(&ctx).map(|s| s.whole());
                    let halo = match spec {
                        Some(g) if g != full => {
                            // The filter of the simulations' resampling: 3.5 σ covers the
                            // exact and the box-blur kernels, plus one cell
                            // for bilinear sampling and one for rounding.
                            let sigma = 0.5 * g.cell_size_m()[0] / cell;
                            (3.5 * sigma).ceil() as u32 + 2
                        }
                        _ => 0,
                    };
                    (spec, halo)
                }
            };
            drop(ctx);
            nodes.insert(
                id.to_string(),
                Planned {
                    step,
                    global,
                    reach,
                    need: 0,
                    world_spec,
                    halo,
                    wave: 0,
                },
            );
        }

        // Needs flow upstream: consumers first.
        for id in order.iter().rev() {
            let p = &nodes[id];
            let upstream_need = (p.need + p.reach).max(p.halo);
            for link in graph.links().iter().filter(|l| &l.to.0 == id) {
                if let Some(u) = nodes.get_mut(&link.from.0) {
                    u.need = u.need.max(upstream_need);
                }
            }
        }

        // Waves of global nodes: after every global node upstream.
        let mut after: BTreeMap<String, usize> = BTreeMap::new(); // globals upstream, inclusive
        for id in &order {
            let upstream = graph
                .links()
                .iter()
                .filter(|l| &l.to.0 == id)
                .filter_map(|l| after.get(&l.from.0).copied())
                .max()
                .unwrap_or(0);
            let p = nodes.get_mut(id).expect("planned");
            if p.global {
                p.wave = upstream;
                after.insert(id.clone(), upstream + 1);
            } else {
                after.insert(id.clone(), upstream);
            }
        }
        let mut waves: Vec<Vec<String>> = Vec::new();
        for id in &order {
            let p = &nodes[id];
            if p.global {
                if waves.len() <= p.wave {
                    waves.resize(p.wave + 1, Vec::new());
                }
                waves[p.wave].push(id.clone());
            }
        }

        Ok(Self {
            graph,
            world,
            full,
            order,
            nodes,
            waves,
            tiles: tiles(full, tile_size),
        })
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Number of passes over all tiles: one per wave of global nodes, plus
    /// the final one.
    pub fn passes(&self) -> usize {
        self.waves.len() + 1
    }

    /// The grid `id` is evaluated over for `tile`.
    fn region(&self, id: &str, tile: &Tile) -> GridSpec {
        let m = self.nodes[id].margin();
        let o = tile.core.offset();
        let (i0, j0) = (o[0].saturating_sub(m), o[1].saturating_sub(m));
        let i1 = (o[0] + tile.core.width + m).min(self.full.width);
        let j1 = (o[1] + tile.core.height + m).min(self.full.height);
        self.full.window(i0, j0, i1 - i0, j1 - j0)
    }

    /// Evaluate the nodes in `wanted` (and what they need) over one tile.
    /// Global nodes upstream must have their world pass in `passes`.
    fn eval_tile(
        &self,
        tile: &Tile,
        wanted: &BTreeSet<String>,
        passes: &BTreeMap<String, (WorldPass, CacheKey)>,
        opts: &EvalOptions,
    ) -> Result<BTreeMap<String, (Outputs, CacheKey)>> {
        let closure = self.upstream_of(wanted);
        let order: Vec<String> = self
            .order
            .iter()
            .filter(|id| closure.contains(*id))
            .cloned()
            .collect();
        let mut uses = consumer_counts(self.graph, &order);
        for id in wanted {
            *uses.entry(id.clone()).or_default() += 1; // never released
        }
        let mut results: BTreeMap<String, (Outputs, CacheKey)> = BTreeMap::new();
        let no_progress = |_: f32| {};
        for id in &order {
            let p = &self.nodes[id];
            let spec = self.region(id, tile);
            let (inputs, keys) = p.step.gather(self.graph, self.world, spec, |from| {
                results.get(from).map(|(o, k)| (o, *k))
            })?;
            let result = match p.global {
                false => p
                    .step
                    .run(self.world, spec, inputs, &keys, opts, &no_progress, None)?,
                true => {
                    let (pass, pass_key) = passes.get(id).ok_or_else(|| {
                        CoreError::Project(format!("tiled build: node {id} has no world pass"))
                    })?;
                    let kind = p.step.kind;
                    let finish = |ctx: &crate::node::EvalContext| kind.finish_tile(ctx, pass);
                    let compute = Compute {
                        tag: format!("tile-of:{pass_key:032x}"),
                        run: &finish,
                    };
                    p.step.run(
                        self.world,
                        spec,
                        inputs,
                        &keys,
                        opts,
                        &no_progress,
                        Some(&compute),
                    )?
                }
            };
            release_inputs(self.graph, id, &mut uses, &mut results);
            results.insert(id.clone(), result);
        }
        results.retain(|id, _| wanted.contains(id));
        Ok(results)
    }

    /// `ids` and every planned node upstream of them.
    fn upstream_of(&self, ids: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for id in ids {
            out.insert(id.clone());
            out.extend(self.graph.upstream(id));
        }
        out.retain(|id| self.nodes.contains_key(id));
        out
    }

    /// Keys identifying each node's result over the whole build without
    /// evaluating it, for caching world passes across builds.
    fn structural_keys(&self) -> BTreeMap<String, CacheKey> {
        let mut keys: BTreeMap<String, CacheKey> = BTreeMap::new();
        for id in &self.order {
            let p = &self.nodes[id];
            let node = p.step.node;
            let mut text = format!(
                "structural|{}@{}|seed={}|grid={:?}|world={:?}|params={}",
                node.type_id,
                p.step.kind.schema().type_version,
                p.step.seed,
                self.full,
                self.world,
                serde_json::to_string(&node.params).unwrap_or_default(),
            );
            for link in self.graph.links().iter().filter(|l| &l.to.0 == id) {
                if let Some(k) = keys.get(&link.from.0) {
                    text.push_str(&format!("|{}={k:032x}.{}", link.to.1, link.from.1));
                }
            }
            let salt = p.step.kind.cache_salt(&node.params, None);
            text.push_str(&salt);
            keys.insert(id.clone(), key_of(&text));
        }
        keys
    }

    /// Run every wave of world passes, then evaluate `targets` (node, port)
    /// tile by tile, handing each tile's core of each target to `sink`.
    pub fn run(
        &self,
        targets: &[(String, String)],
        opts: &EvalOptions,
        sink: &mut dyn FnMut(&Tile, &str, &str, Value) -> Result<()>,
    ) -> Result<()> {
        let passes_total = self.passes() as f32;
        let tiles_total = self.tiles.len() as f32;
        let report = |pass: usize, tile: usize| {
            if let Some(p) = opts.progress {
                p((pass as f32 + tile as f32 / tiles_total) / passes_total);
            }
        };
        let structural = self.structural_keys();
        let mut world_passes: BTreeMap<String, (WorldPass, CacheKey)> = BTreeMap::new();

        for (w, wave) in self.waves.iter().enumerate() {
            let mut streams: BTreeMap<String, Streams> = wave
                .iter()
                .map(|id| Ok((id.clone(), Streams::new(self, id)?)))
                .collect::<Result<_>>()?;
            let wanted: BTreeSet<String> = wave
                .iter()
                .flat_map(|id| self.graph.links().iter().filter(move |l| &l.to.0 == id))
                .map(|l| l.from.0.clone())
                .collect();
            for (t, tile) in self.tiles.iter().enumerate() {
                report(w, t);
                let results = self.eval_tile(tile, &wanted, &world_passes, opts)?;
                for (id, s) in streams.iter_mut() {
                    s.add_tile(self, id, tile, &results)?;
                }
            }
            for id in wave {
                let s = streams.remove(id).expect("made above");
                let pass = s.finish(self, id, &structural, opts)?;
                world_passes.insert(id.clone(), pass);
            }
        }

        let wanted: BTreeSet<String> = targets.iter().map(|(n, _)| n.clone()).collect();
        let last = self.waves.len();
        for (t, tile) in self.tiles.iter().enumerate() {
            report(last, t);
            let results = self.eval_tile(tile, &wanted, &world_passes, opts)?;
            for (node, port) in targets {
                let value = results.get(node).and_then(|(o, _)| o.get(port)).ok_or_else(|| {
                    CoreError::PortNotFound {
                        node: node.clone(),
                        port: port.clone(),
                    }
                })?;
                let value = match value {
                    Value::Points(p) => {
                        let mut own = crate::points::PointSet::new(tile.core, "");
                        own.species = p.species.clone();
                        for q in p.iter().filter(|q| tile.owns(q.x_m as f64, q.y_m as f64)) {
                            own.push(q);
                        }
                        Value::Points(Arc::new(own))
                    }
                    v => v.crop(tile.core),
                };
                sink(tile, node, port, value)?;
            }
        }
        if let Some(p) = opts.progress {
            p(1.0);
        }
        Ok(())
    }
}

/// One input of a global node being streamed onto its world-pass grid.
struct Stream {
    ty: PortType,
    /// The world-pass grid being filled (if the node has one).
    grid: Option<Vec<f32>>,
    /// Lowest and highest value seen so far.
    range: (f32, f32),
}

/// The connected inputs of one global node, by port.
struct Streams {
    ports: BTreeMap<String, Stream>,
}

impl Streams {
    fn new(plan: &TiledPlan, id: &str) -> Result<Self> {
        let p = &plan.nodes[id];
        let mut ports = BTreeMap::new();
        for port in &p.step.ports {
            if plan.graph.link_into(id, &port.key).is_none() {
                continue;
            }
            if !matches!(port.ty, PortType::Heightfield | PortType::Mask) {
                return Err(CoreError::Project(format!(
                    "node {id}: its {} input can't be used in a tiled build",
                    port.label
                )));
            }
            let grid = p.world_spec.map(|g| vec![0.0f32; g.len()]);
            let range = (f32::INFINITY, f32::NEG_INFINITY);
            ports.insert(port.key.clone(), Stream { ty: port.ty, grid, range });
        }
        Ok(Self { ports })
    }

    /// Add one tile's inputs: their range over the core, and the world-pass
    /// samples the tile owns.
    fn add_tile(
        &mut self,
        plan: &TiledPlan,
        id: &str,
        tile: &Tile,
        results: &BTreeMap<String, (Outputs, CacheKey)>,
    ) -> Result<()> {
        let p = &plan.nodes[id];
        for (key, Stream { ty, grid, range }) in self.ports.iter_mut() {
            let link = plan.graph.link_into(id, key).expect("connected");
            let value = results
                .get(&link.from.0)
                .and_then(|(o, _)| o.get(&link.from.1))
                .ok_or_else(|| CoreError::PortNotFound {
                    node: link.from.0.clone(),
                    port: link.from.1.clone(),
                })?
                .convert(*ty, plan.world);
            let region = value.grid();
            let (lo, hi) = region.crop(tile.core).min_max();
            *range = (range.0.min(lo), range.1.max(hi));

            let (Some(grid), Some(g)) = (grid.as_mut(), p.world_spec) else {
                continue;
            };
            if g == plan.full {
                // Same grid: copy the core.
                let core = region.crop(tile.core);
                let o = tile.core.offset();
                for j in 0..core.spec.height as usize {
                    let row = &core.data[j * core.spec.width as usize..(j + 1) * core.spec.width as usize];
                    let start = (o[1] as usize + j) * g.width as usize + o[0] as usize;
                    grid[start..start + row.len()].copy_from_slice(row);
                }
                continue;
            }
            // Filter as `resample_into` does, then sample the world-pass
            // positions this tile owns.
            let smooth = gaussian_blur(region, 0.5 * g.cell_size_m()[0]);
            let (x0, y0) = (tile.core.origin_m[0], tile.core.origin_m[1]);
            let (x1, y1) = (x0 + tile.core.extent_m[0], y0 + tile.core.extent_m[1]);
            let first_col = (g.column_at(x0).floor().max(0.0) as u32).min(g.width - 1);
            let first_row = (g.row_at(y0).floor().max(0.0) as u32).min(g.height - 1);
            let last_col = ((g.column_at(x1).ceil() + 1.0) as u32).min(g.width - 1);
            let last_row = ((g.row_at(y1).ceil() + 1.0) as u32).min(g.height - 1);
            for gj in first_row..=last_row {
                let y = g.y_m(gj);
                for gi in first_col..=last_col {
                    let x = g.x_m(gi);
                    if tile.owns(x, y) {
                        grid[gj as usize * g.width as usize + gi as usize] = smooth.sample_bilinear_m(x, y);
                    }
                }
            }
        }
        Ok(())
    }

    /// Run the node's world pass on the streamed inputs.
    fn finish(
        self,
        plan: &TiledPlan,
        id: &str,
        structural: &BTreeMap<String, CacheKey>,
        opts: &EvalOptions,
    ) -> Result<(WorldPass, CacheKey)> {
        let p = &plan.nodes[id];
        let mut inputs = BTreeMap::new();
        let mut ranges = BTreeMap::new();
        let mut keys = Vec::new();
        for (key, Stream { ty, grid, range }) in self.ports {
            let link = plan.graph.link_into(id, &key).expect("connected");
            ranges.insert(key.clone(), range);
            let source = structural.get(&link.from.0).copied().unwrap_or_default();
            let stream_key = key_of(&format!("stream|{source:032x}|{:?}", p.world_spec));
            keys.push((key.clone(), stream_key, link.from.1.clone()));
            if let (Some(data), Some(spec)) = (grid, p.world_spec) {
                let grid = Arc::new(Grid { spec, data });
                let value = match ty {
                    PortType::Mask => Value::Mask(grid),
                    _ => Value::Heightfield(grid),
                };
                inputs.insert(key, value);
            }
        }
        let no_progress = |_: f32| {};
        let (outputs, key) = match p.world_spec {
            Some(spec) => p
                .step
                .run(plan.world, spec, inputs.clone(), &keys, opts, &no_progress, None)?,
            None => {
                let text = keys
                    .iter()
                    .map(|(k, s, o)| format!("{k}={s:032x}.{o}"))
                    .collect::<Vec<_>>()
                    .join("|");
                let ranges_text = format!("{ranges:?}");
                (
                    Outputs::new(),
                    key_of(&format!("ranges-only|{id}|{text}|{ranges_text}")),
                )
            }
        };
        Ok((
            WorldPass {
                spec: p.world_spec,
                inputs,
                outputs,
                ranges,
            },
            key,
        ))
    }
}

/// Evaluate `targets` (node, port) over the build grid `full` in tiles of
/// `tile_size` samples, handing each tile's share of each target to `sink`.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_tiled(
    graph: &Graph,
    registry: &NodeRegistry,
    world: &World,
    full: GridSpec,
    targets: &[(String, String)],
    tile_size: u32,
    opts: &EvalOptions,
    sink: &mut dyn FnMut(&Tile, &str, &str, Value) -> Result<()>,
) -> Result<()> {
    let ids: Vec<&str> = targets.iter().map(|(n, _)| n.as_str()).collect();
    let plan = TiledPlan::new(graph, registry, world, full, &ids, tile_size)?;
    plan.run(targets, opts, sink)
}
