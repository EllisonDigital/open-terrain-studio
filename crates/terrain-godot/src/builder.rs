use std::sync::Arc;
use std::time::Instant;

use godot::prelude::*;
use terrain_core::{EvalOptions, Grid, GridSpec, PortType, evaluate_node};

use crate::convert::put;
use crate::jobs::Job;
use crate::preview::{PreviewData, TerrainPreview};
use crate::project::TerrainProject;
use crate::{cache, registry};

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
            Ok(PreviewData {
                grid,
                port_type: ty,
                base,
                computed: cache().stats().misses - misses_before,
                millis: started.elapsed().as_secs_f64() * 1000.0,
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

    /// Empty the result cache (e.g. to measure cold performance).
    #[func]
    fn clear_cache() {
        cache().clear();
    }
}
