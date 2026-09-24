use std::sync::Arc;

use godot::prelude::*;
use terrain_core::{EvalOptions, Grid, GridSpec, PortType, evaluate_node};

use crate::jobs::Job;
use crate::preview::TerrainPreview;
use crate::project::TerrainProject;
use crate::registry;

type PreviewResult = (Arc<Grid>, PortType);

/// Evaluates nodes on a worker thread. Add it to the scene tree: results are
/// delivered by signals from `_process`, on the main thread.
///
/// Every request gets a new generation number; results from older requests are
/// discarded, so rapid parameter edits never show stale terrain.
#[derive(GodotClass)]
#[class(base=Node)]
pub struct TerrainBuilder {
    generation: i64,
    job: Option<(Job<PreviewResult>, String, String)>,
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
                Ok((grid, ty)) => {
                    let preview = TerrainPreview::create(grid, ty, &node_id, &port, generation);
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
        let snapshot = project.bind().snapshot();
        let (node, port_s) = (node_id.to_string(), port.to_string());
        let (n2, p2) = (node.clone(), port_s.clone());
        let job = Job::spawn(generation, move |cancel, progress| {
            let spec =
                GridSpec::full_world(&snapshot.world, resolution.max(2) as u32).map_err(|e| e.to_string())?;
            let outputs = evaluate_node(
                &snapshot.graph,
                registry(),
                &snapshot.world,
                spec,
                &n2,
                &EvalOptions {
                    cancel: Some(cancel),
                    progress: Some(progress),
                },
            )
            .map_err(|e| e.to_string())?;
            let value = outputs
                .get(&p2)
                .ok_or_else(|| format!("node {n2} has no output '{p2}'"))?;
            Ok((value.grid().clone(), value.port_type()))
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
}
