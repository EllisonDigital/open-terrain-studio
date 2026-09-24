use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use godot::prelude::*;
use terrain_core::Project;

use crate::graph::TerrainGraph;
use crate::registry;

/// Project state shared by `TerrainProject` and its `TerrainGraph` view.
pub struct State {
    pub project: Project,
    /// Unsaved changes exist.
    pub modified: bool,
}

pub type Shared = Arc<Mutex<State>>;

pub fn new_shared(project: Project) -> Shared {
    Arc::new(Mutex::new(State {
        project,
        modified: false,
    }))
}

/// Lock the shared state, recovering from a poisoned lock (a panicked worker
/// must never make the project unusable).
pub fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// A terrain project: world settings, graph, build settings and editor state.
#[derive(GodotClass)]
#[class(base=RefCounted)]
pub struct TerrainProject {
    pub(crate) shared: Shared,
    last_error: GString,
    warnings: PackedStringArray,
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for TerrainProject {
    fn init(base: Base<RefCounted>) -> Self {
        Self {
            shared: new_shared(Project::default()),
            last_error: GString::new(),
            warnings: PackedStringArray::new(),
            base,
        }
    }
}

#[godot_api]
impl TerrainProject {
    /// Engine version, e.g. "0.1.0".
    #[func]
    fn get_app_version() -> GString {
        terrain_core::APP_VERSION.into()
    }

    /// Project file extension without the dot ("otstudio").
    #[func]
    fn get_file_extension() -> GString {
        terrain_core::project::EXTENSION.into()
    }

    /// Replace everything with an empty project.
    #[func]
    fn new_project(&mut self) {
        let mut s = lock(&self.shared);
        s.project = Project::default();
        s.modified = false;
        self.warnings = PackedStringArray::new();
    }

    /// Load a project file. Returns false on error (see `get_last_error`).
    /// Non-fatal problems are listed by `get_warnings`.
    #[func]
    fn load(&mut self, path: GString) -> bool {
        match Project::load(Path::new(&path.to_string()), registry()) {
            Ok((project, warnings)) => {
                let mut s = lock(&self.shared);
                s.project = project;
                s.modified = false;
                self.warnings = warnings.iter().map(|w| GString::from(w.as_str())).collect();
                true
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    #[func]
    fn save(&mut self, path: GString) -> bool {
        let result = {
            let s = lock(&self.shared);
            s.project.save(Path::new(&path.to_string()))
        };
        match result {
            Ok(()) => {
                lock(&self.shared).modified = false;
                true
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    #[func]
    fn get_last_error(&self) -> GString {
        self.last_error.clone()
    }

    #[func]
    fn get_warnings(&self) -> PackedStringArray {
        self.warnings.clone()
    }

    #[func]
    fn is_modified(&self) -> bool {
        lock(&self.shared).modified
    }

    /// Mark the project as having no unsaved changes (e.g. after building a starter graph).
    #[func]
    fn clear_modified(&mut self) {
        lock(&self.shared).modified = false;
    }

    /// The node graph of this project (a live view; edits apply immediately).
    #[func]
    fn get_graph(&self) -> Gd<TerrainGraph> {
        TerrainGraph::for_project(self.shared.clone())
    }

    // ---- world ----------------------------------------------------------

    /// World width and depth in metres (square worlds in v0.1).
    #[func]
    fn get_world_size(&self) -> f64 {
        lock(&self.shared).project.world.size_m[0]
    }

    #[func]
    fn set_world_size(&mut self, size_m: f64) -> bool {
        if !(size_m.is_finite() && size_m >= 1.0) {
            return self.fail("world size must be at least 1 m".into());
        }
        let mut s = lock(&self.shared);
        s.project.world.size_m = [size_m, size_m];
        s.modified = true;
        true
    }

    #[func]
    fn get_height_min(&self) -> f32 {
        lock(&self.shared).project.world.height_range_m[0]
    }

    #[func]
    fn get_height_max(&self) -> f32 {
        lock(&self.shared).project.world.height_range_m[1]
    }

    #[func]
    fn set_height_range(&mut self, min_m: f32, max_m: f32) -> bool {
        if !(min_m.is_finite() && max_m.is_finite() && max_m > min_m) {
            return self.fail("height range max must be greater than min".into());
        }
        let mut s = lock(&self.shared);
        s.project.world.height_range_m = [min_m, max_m];
        s.modified = true;
        true
    }

    #[func]
    fn get_seed(&self) -> i64 {
        lock(&self.shared).project.world.seed as i64
    }

    #[func]
    fn set_seed(&mut self, seed: i64) {
        let mut s = lock(&self.shared);
        s.project.world.seed = seed as u64;
        s.modified = true;
    }

    // ---- build settings -------------------------------------------------

    #[func]
    fn get_build_resolution(&self) -> i32 {
        lock(&self.shared).project.build.resolution as i32
    }

    #[func]
    fn set_build_resolution(&mut self, resolution: i32) {
        let mut s = lock(&self.shared);
        s.project.build.resolution = resolution.clamp(2, terrain_core::grid::MAX_RESOLUTION as i32) as u32;
        s.modified = true;
    }

    #[func]
    fn get_build_folder(&self) -> GString {
        lock(&self.shared).project.build.folder.as_str().into()
    }

    #[func]
    fn set_build_folder(&mut self, folder: GString) {
        let mut s = lock(&self.shared);
        s.project.build.folder = folder.to_string();
        s.modified = true;
    }

    // ---- editor state -----------------------------------------------------

    /// Editor state (viewed node, camera, ...) as a JSON string.
    #[func]
    fn get_ui_state(&self) -> GString {
        lock(&self.shared).project.ui.to_string().as_str().into()
    }

    /// Store editor state (a JSON string). Does not mark the project modified.
    #[func]
    fn set_ui_state(&mut self, json: GString) -> bool {
        match serde_json::from_str(&json.to_string()) {
            Ok(v) => {
                lock(&self.shared).project.ui = v;
                true
            }
            Err(e) => self.fail(format!("invalid UI state JSON: {e}")),
        }
    }
}

impl TerrainProject {
    fn fail(&mut self, message: String) -> bool {
        godot_warn!("TerrainProject: {message}");
        self.last_error = message.as_str().into();
        false
    }

    /// A consistent copy of the project for a background job.
    pub(crate) fn snapshot(&self) -> Project {
        lock(&self.shared).project.clone()
    }
}
