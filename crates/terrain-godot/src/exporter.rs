use std::path::PathBuf;

use godot::prelude::*;
use terrain_core::EvalOptions;
use terrain_core::export::{ExportFormat, ExportRequest, build_marked, export_node};

use crate::gpu;
use crate::jobs::Job;
use crate::project::TerrainProject;
use crate::{cache, registry};

/// Writes node outputs to disk (EXR 32-bit, PNG 16-bit, plus build.json) on a
/// worker thread: one viewed output (`request_export`) or every output marked
/// for export (`request_build`). Add it to the scene tree; results arrive by
/// signal.
#[derive(GodotClass)]
#[class(base=Node)]
pub struct TerrainExporter {
    generation: i64,
    job: Option<Job<Vec<PathBuf>>>,
    base: Base<Node>,
}

#[godot_api]
impl INode for TerrainExporter {
    fn init(base: Base<Node>) -> Self {
        Self {
            generation: 0,
            job: None,
            base,
        }
    }

    fn process(&mut self, _delta: f64) {
        let Some(job) = &self.job else { return };
        let (progress, done) = job.poll();
        if let Some(p) = progress {
            self.signals().progress().emit(p);
        }
        if let Some(result) = done {
            self.job = None;
            match result {
                Ok(paths) => {
                    let files: PackedStringArray = paths
                        .iter()
                        .map(|p| GString::from(p.to_string_lossy().as_ref()))
                        .collect();
                    self.signals()
                        .export_finished()
                        .emit(true, &GString::new(), &files);
                }
                Err(message) => {
                    self.signals().export_finished().emit(
                        false,
                        &GString::from(message.as_str()),
                        &PackedStringArray::new(),
                    );
                }
            }
        }
    }
}

#[godot_api]
impl TerrainExporter {
    #[signal]
    fn progress(fraction: f32);

    /// `files` lists every file written (images and build.json).
    #[signal]
    fn export_finished(ok: bool, message: GString, files: PackedStringArray);

    /// Export `node_id.port` at `resolution` into `folder`.
    /// `formats`: any of "exr32", "png16". Returns false if the request is invalid.
    #[func]
    fn request_export(
        &mut self,
        project: Gd<TerrainProject>,
        node_id: GString,
        port: GString,
        resolution: i32,
        folder: GString,
        formats: PackedStringArray,
    ) -> bool {
        if self.job.is_some() {
            godot_warn!("TerrainExporter: an export is already running");
            return false;
        }
        let mut fmts = Vec::new();
        for f in formats.as_slice() {
            match ExportFormat::parse(&f.to_string()) {
                Some(x) => fmts.push(x),
                None => {
                    godot_warn!("TerrainExporter: unknown format '{f}'");
                    return false;
                }
            }
        }
        if fmts.is_empty() {
            godot_warn!("TerrainExporter: no formats selected");
            return false;
        }
        self.generation += 1;
        let (snapshot, base_dir) = project.bind().snapshot();
        let (node, port, folder) = (
            node_id.to_string(),
            port.to_string(),
            PathBuf::from(folder.to_string()),
        );
        self.job = Some(Job::spawn(self.generation, move |cancel, progress| {
            build_pool().install(|| {
                export_node(
                    &snapshot,
                    registry(),
                    &ExportRequest {
                        node: &node,
                        port: &port,
                        resolution: resolution.max(2) as u32,
                        folder: &folder,
                        formats: &fmts,
                    },
                    &EvalOptions {
                        cancel: Some(cancel),
                        progress: Some(progress),
                        cache: Some(cache()),
                        base_dir: base_dir.as_deref(),
                        gpu: gpu::for_build().as_ref(),
                    },
                )
                .map_err(|e| e.to_string())
            })
        }));
        true
    }

    /// Build every output marked for export at `resolution` into `folder`
    /// (relative folders are inside the project's folder). Returns false if an
    /// export or build is already running.
    #[func]
    fn request_build(&mut self, project: Gd<TerrainProject>, resolution: i32, folder: GString) -> bool {
        if self.job.is_some() {
            godot_warn!("TerrainExporter: an export is already running");
            return false;
        }
        self.generation += 1;
        let (snapshot, base_dir) = project.bind().snapshot();
        let folder = PathBuf::from(folder.to_string());
        self.job = Some(Job::spawn(self.generation, move |cancel, progress| {
            build_pool().install(|| {
                build_marked(
                    &snapshot,
                    registry(),
                    resolution.max(2) as u32,
                    &folder,
                    &EvalOptions {
                        cancel: Some(cancel),
                        progress: Some(progress),
                        cache: Some(cache()),
                        base_dir: base_dir.as_deref(),
                        gpu: gpu::for_build().as_ref(),
                    },
                )
                .map_err(|e| e.to_string())
            })
        }));
        true
    }

    #[func]
    fn cancel(&mut self) {
        // Keep the job until its worker stops (it cleans up its temporary
        // files); `export_finished` then reports it as cancelled.
        if let Some(job) = &self.job {
            job.cancel();
        }
    }

    #[func]
    fn is_busy(&self) -> bool {
        self.job.is_some()
    }
}

/// Threads for exports and builds: all but two cores, so previews stay
/// responsive while a long build runs in the background.
fn build_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        rayon::ThreadPoolBuilder::new()
            .num_threads(cores.saturating_sub(2).max(1))
            .thread_name(|i| format!("terrain-build-{i}"))
            .build()
            .expect("build thread pool")
    })
}
