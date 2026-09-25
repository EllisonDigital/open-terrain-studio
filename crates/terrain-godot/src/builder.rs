use std::sync::Arc;
use std::time::Instant;

use godot::prelude::*;
use terrain_core::{EvalOptions, Grid, GridSpec, PortType, evaluate_node};

use crate::convert::put;
use crate::gpu;
use crate::jobs::Job;
use crate::preview::{PreviewData, TerrainPreview};
use crate::project::TerrainProject;
use crate::{cache, registry};

/// Water level marking dry ground in [`PreviewData::water`].
pub const DRY: f32 = -1.0e6;
/// Water shallower than this isn't drawn.
const WATER_MIN_DEPTH_M: f32 = 0.01;

/// Evaluates nodes on a worker thread. Add it to the scene tree: results are
/// delivered by signals from `_process`, on the main thread.
///
/// Every request gets a new generation number; results from older requests are
/// discarded, so rapid parameter edits never show stale terrain. Results are
/// cached, so only nodes whose inputs or settings changed are recomputed.
#[derive(GodotClass)]
#[class(base=Node)]
pub struct TerrainBuilder {
    generation: i64,
    job: Option<(Job<PreviewData>, String, String)>,
    base: Base<Node>,
}

#[godot_api]
impl INode for TerrainBuilder {
    fn init(base: Base<Node>) -> Self {
        Self {
            generation: 0,
            job: None,
            base,
        }
    }

    fn process(&mut self, _delta: f64) {
        let Some((job, node_id, port)) = &self.job else {
            return;
        };
        let generation = job.generation;
        let (progress, done) = job.poll();
        let (node_id, port) = (node_id.clone(), port.clone());
        if let Some(p) = progress {
            self.signals().progress().emit(generation, p);
        }
        if let Some(result) = done {
            self.job = None;
            match result {
                Ok(data) => {
                    let preview = TerrainPreview::create(data, &node_id, &port, generation);
                    self.signals().preview_ready().emit(&preview);
                }
                Err(message) => {
                    self.signals()
                        .preview_failed()
                        .emit(generation, &GString::from(message.as_str()));
                }
            }
        }
    }
}

#[godot_api]
impl TerrainBuilder {
    /// Progress of the current request, 0..1.
    #[signal]
    fn progress(generation: i64, fraction: f32);

    /// The latest request finished.
    #[signal]
    fn preview_ready(preview: Gd<TerrainPreview>);

    /// The latest request failed (e.g. a required input is not connected).
    #[signal]
    fn preview_failed(generation: i64, message: GString);

    /// Evaluate `node_id`'s output `port` over the whole world at `resolution`.
    /// If the output is a mask, the terrain it was computed from (the nearest
    /// heightfield upstream) is evaluated too, for draping the mask over it.
    /// Cancels any running request. Returns the request's generation number.
    #[func]
    fn request_preview(
        &mut self,
        project: Gd<TerrainProject>,
        node_id: GString,
        port: GString,
        resolution: i32,
    ) -> i64 {
        if let Some((job, _, _)) = &self.job {
            job.cancel();
        }
        self.generation += 1;
        let generation = self.generation;
        let (snapshot, base_dir) = project.bind().snapshot();
        let (node, port_s) = (node_id.to_string(), port.to_string());
        let (n2, p2) = (node.clone(), port_s.clone());
        let job = Job::spawn(generation, move |cancel, progress| {
            let started = Instant::now();
            let gpu = gpu::for_preview();
            let misses_before = cache().stats().misses;
            let spec =
                GridSpec::full_world(&snapshot.world, resolution.max(2) as u32).map_err(|e| e.to_string())?;
            let eval =
                |node: &str, port: &str, progress: Option<&(dyn Fn(f32) + Sync)>| -> Result<_, String> {
                    let outputs = evaluate_node(
                        &snapshot.graph,
                        registry(),
                        &snapshot.world,
                        spec,
                        node,
                        &EvalOptions {
                            cancel: Some(cancel),
                            progress,
                            cache: Some(cache()),
                            base_dir: base_dir.as_deref(),
                            gpu: gpu.as_ref(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    let value = outputs
                        .get(port)
                        .ok_or_else(|| format!("node {node} has no output '{port}'"))?;
                    Ok((value.grid().clone(), value.port_type()))
                };
            let (grid, ty) = eval(&n2, &p2, Some(progress))?;
            // The base terrain is upstream, so it is normally already cached.
            let base: Option<(Arc<Grid>, String)> = if ty == PortType::Mask {
                snapshot
                    .graph
                    .base_heightfield(registry(), &n2)
                    .and_then(|(bn, bp)| eval(&bn, &bp, None).ok().map(|(g, _)| (g, bn)))
            } else {
                None
            };
            // Water to draw over a terrain: the highest water surface of the
            // Rivers, Lakes and Sea nodes it was made with, where they have water.
            let water = if ty == PortType::Heightfield {
                let mut level: Option<Vec<f32>> = None;
                for source in snapshot.graph.water_sources(registry(), &n2) {
                    let (Ok((height, _)), Ok((surface, _))) = (
                        eval(&source, "height", None),
                        eval(&source, "water_surface", None),
                    ) else {
                        continue;
                    };
                    let level = level.get_or_insert_with(|| vec![DRY; grid.data.len()]);
                    for ((l, s), h) in level.iter_mut().zip(&surface.data).zip(&height.data) {
                        if *s > *h + WATER_MIN_DEPTH_M {
                            *l = l.max(*s);
                        }
                    }
                }
                level.map(|data| Arc::new(Grid { spec, data }))
            } else {
                None
            };
            // Snow to paint on a terrain: the most snow of the Snow nodes it
            // was made with.
            let snow = if ty == PortType::Heightfield {
                let mut cover: Option<Vec<f32>> = None;
                for source in snapshot.graph.snow_sources(registry(), &n2) {
                    let Ok((snow, _)) = eval(&source, "snow", None) else {
                        continue;
                    };
                    let cover = cover.get_or_insert_with(|| vec![0.0; grid.data.len()]);
                    for (c, s) in cover.iter_mut().zip(&snow.data) {
                        *c = c.max(*s);
                    }
                }
                cover.map(|data| Arc::new(Grid { spec, data }))
            } else {
                None
            };
            Ok(PreviewData {
                grid,
                port_type: ty,
                base,
                water,
                snow,
                computed: cache().stats().misses - misses_before,
                millis: started.elapsed().as_secs_f64() * 1000.0,
                gpu: gpu.map(|g| g.name()),
            })
        });
        self.job = Some((job, node, port_s));
        generation
    }

    /// Stop the running request, if any. No signal is emitted for it.
    #[func]
    fn cancel(&mut self) {
        if let Some((job, _, _)) = self.job.take() {
            job.cancel();
        }
    }

    #[func]
    fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// Cache counters: hits, misses (nodes computed), evictions, entries, megabytes.
    #[func]
    fn get_cache_stats() -> VarDictionary {
        let s = cache().stats();
        let mut d = VarDictionary::new();
        put(&mut d, "hits", s.hits as i64);
        put(&mut d, "misses", s.misses as i64);
        put(&mut d, "evictions", s.evictions as i64);
        put(&mut d, "entries", s.entries as i64);
        put(&mut d, "megabytes", s.bytes as f64 / (1024.0 * 1024.0));
        d
    }

    /// The compute device: `state` ("starting", "ready" or "unavailable"),
    /// `name`, `reason` (why there is none), `force_cpu`, `builds_on_gpu`,
    /// and counters `nodes` (node runs on the GPU), `fallbacks` (GPU runs that
    /// failed and used the CPU) and `last_error`. Starts the device if needed.
    #[func]
    fn get_gpu_status() -> VarDictionary {
        gpu::start();
        let mut d = VarDictionary::new();
        put(&mut d, "force_cpu", gpu::force_cpu());
        put(&mut d, "builds_on_gpu", gpu::builds_on_gpu());
        match gpu::status() {
            None => put(&mut d, "state", "starting"),
            Some(Err(reason)) => {
                put(&mut d, "state", "unavailable");
                put(&mut d, "reason", GString::from(reason.as_str()));
            }
            Some(Ok(g)) => {
                let stats = g.stats();
                put(&mut d, "state", "ready");
                put(&mut d, "name", GString::from(g.name().as_str()));
                put(&mut d, "nodes", stats.nodes as i64);
                put(&mut d, "fallbacks", stats.fallbacks as i64);
                put(
                    &mut d,
                    "last_error",
                    GString::from(stats.last_error.unwrap_or_default().as_str()),
                );
            }
        }
        d
    }

    /// Run every node on the CPU, even with a GPU ("Force CPU", for
    /// debugging). Affects the next request.
    #[func]
    fn set_force_cpu(on: bool) {
        gpu::set_force_cpu(on);
    }

    /// Let builds and exports use the GPU. Off by default: builds then are
    /// bit-exact CPU results, identical on every machine.
    #[func]
    fn set_builds_on_gpu(on: bool) {
        gpu::set_builds_on_gpu(on);
    }

    /// Compare every GPU kernel with its CPU version at `resolution`² (blocks;
    /// for tests). One dictionary per output: case, port, max_error,
    /// outliers, tolerance, tolerance_share, cpu_ms, gpu_ms, passed, error,
    /// worst (where the largest difference is).
    /// Empty if there is no GPU.
    #[func]
    fn run_gpu_check(resolution: i32) -> VarArray {
        let mut arr = VarArray::new();
        let Ok(g) = gpu::device() else { return arr };
        for r in terrain_nodes::gpu_check::check_all(g, resolution.max(3) as u32) {
            let mut d = VarDictionary::new();
            put(&mut d, "case", GString::from(r.case.as_str()));
            put(&mut d, "port", GString::from(r.port.as_str()));
            put(&mut d, "max_error", r.max_error);
            put(&mut d, "outliers", r.outliers);
            put(&mut d, "tolerance", r.tolerance.0);
            put(&mut d, "tolerance_share", r.tolerance.1);
            put(&mut d, "cpu_ms", r.cpu_ms);
            put(&mut d, "gpu_ms", r.gpu_ms);
            put(&mut d, "passed", r.passed);
            put(&mut d, "worst", GString::from(r.worst.as_str()));
            put(
                &mut d,
                "error",
                GString::from(r.error.unwrap_or_default().as_str()),
            );
            arr.push(&d.to_variant());
        }
        arr
    }

    /// Empty the result cache (e.g. to measure cold performance).
    #[func]
    fn clear_cache() {
        cache().clear();
    }
}
