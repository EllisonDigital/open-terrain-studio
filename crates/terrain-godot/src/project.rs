use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use godot::prelude::*;
use terrain_core::{EditState, History, Project};

use crate::convert::put;
use crate::graph::TerrainGraph;
use crate::registry;

/// Project state shared by `TerrainProject` and its `TerrainGraph` view.
pub struct State {
    pub project: Project,
    pub history: History,
    /// Where the project was last loaded from or saved to.
    pub path: Option<PathBuf>,
}

pub type Shared = Arc<Mutex<State>>;

pub fn new_shared(project: Project) -> Shared {
    Arc::new(Mutex::new(State {
        project,
        history: History::default(),
        path: None,
    }))
}

/// Lock the shared state, recovering from a poisoned lock (a panicked worker
/// must never make the project unusable).
pub fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Seconds since the extension loaded (for merging rapid edits into one undo step).
fn now_s() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

impl State {
    /// Apply an undoable edit. If `f` fails nothing is recorded (and `f` must
    /// leave the project unchanged).
    pub fn edit<R>(
        &mut self,
        label: &str,
        merge_key: Option<&str>,
        f: impl FnOnce(&mut Project) -> terrain_core::error::Result<R>,
    ) -> terrain_core::error::Result<R> {
        let before = EditState::of(&self.project);
        let r = f(&mut self.project)?;
        if EditState::of(&self.project) != before {
            self.history.record(label, merge_key, now_s(), before);
        }
        Ok(r)
    }

    /// The project's folder, for resolving relative paths.
    pub fn base_dir(&self) -> Option<PathBuf> {
        self.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)
    }
}

/// A terrain project: world settings, graph, export marks, build settings,
/// editor state and undo history.
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
    /// Engine version, e.g. "0.2.0".
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
        s.history.reset();
        s.path = None;
        self.warnings = PackedStringArray::new();
    }

    /// Load a project file. Returns false on error (see `get_last_error`).
    /// Non-fatal problems are listed by `get_warnings`.
    #[func]
    fn load(&mut self, path: GString) -> bool {
        let path = PathBuf::from(path.to_string());
        match Project::load(&path, registry()) {
            Ok((project, warnings)) => {
                self.replace(project, Some(path), warnings);
                true
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Load a project from JSON text (e.g. a bundled example). It has no file
    /// yet, so it is untitled and relative file paths can't be resolved.
    #[func]
    fn load_json(&mut self, json: GString) -> bool {
        match Project::from_json(&json.to_string(), registry()) {
            Ok((project, warnings)) => {
                self.replace(project, None, warnings);
                true
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    #[func]
    fn save(&mut self, path: GString) -> bool {
        let path = PathBuf::from(path.to_string());
        let result = lock(&self.shared).project.save(&path);
        match result {
            Ok(()) => {
                let mut s = lock(&self.shared);
                s.history.mark_saved();
                s.path = Some(path);
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

    /// True if there are unsaved changes. Undoing back to the saved state
    /// clears it again.
    #[func]
    fn is_modified(&self) -> bool {
        lock(&self.shared).history.is_modified()
    }

    /// Mark the current state as saved.
    #[func]
    fn clear_modified(&mut self) {
        lock(&self.shared).history.mark_saved();
    }

    /// Forget undo history and mark the current state as saved (e.g. after
    /// building a starter graph).
    #[func]
    fn reset_history(&mut self) {
        lock(&self.shared).history.reset();
    }

    /// The node graph of this project (a live view; edits apply immediately).
    #[func]
    fn get_graph(&self) -> Gd<TerrainGraph> {
        TerrainGraph::for_project(self.shared.clone())
    }

    /// Folder of the project file, or "" if it hasn't been saved.
    #[func]
    fn get_project_dir(&self) -> GString {
        lock(&self.shared)
            .base_dir()
            .map(|d| GString::from(d.to_string_lossy().as_ref()))
            .unwrap_or_default()
    }

    // ---- undo / redo ----------------------------------------------------

    /// Undo the last edit. Returns its label (e.g. "Set Height"), or "" if
    /// there was nothing to undo.
    #[func]
    fn undo(&mut self) -> GString {
        let mut s = lock(&self.shared);
        let current = EditState::of(&s.project);
        match s.history.undo(current) {
            Some((state, label)) => {
                state.apply_to(&mut s.project);
                label.as_str().into()
            }
            None => GString::new(),
        }
    }

    /// Redo the last undone edit. Returns its label, or "".
    #[func]
    fn redo(&mut self) -> GString {
        let mut s = lock(&self.shared);
        let current = EditState::of(&s.project);
        match s.history.redo(current) {
            Some((state, label)) => {
                state.apply_to(&mut s.project);
                label.as_str().into()
            }
            None => GString::new(),
        }
    }

    #[func]
    fn can_undo(&self) -> bool {
        lock(&self.shared).history.can_undo()
    }

    #[func]
    fn can_redo(&self) -> bool {
        lock(&self.shared).history.can_redo()
    }

    #[func]
    fn get_undo_label(&self) -> GString {
        lock(&self.shared).history.undo_label().unwrap_or_default().into()
    }

    #[func]
    fn get_redo_label(&self) -> GString {
        lock(&self.shared).history.redo_label().unwrap_or_default().into()
    }

    /// Group the following edits into one undo step called `label`, until
    /// `end_edit_group` (e.g. deleting several nodes at once).
    #[func]
    fn begin_edit_group(&mut self, label: GString) {
        lock(&self.shared).history.begin_group(&label.to_string());
    }

    #[func]
    fn end_edit_group(&mut self) {
        lock(&self.shared).history.end_group();
    }

    /// Stop the next edit merging into the previous undo step (e.g. when a
    /// slider drag ends).
    #[func]
    fn break_undo_merge(&mut self) {
        lock(&self.shared).history.break_merge();
    }

    // ---- world ----------------------------------------------------------

    /// World width and depth in metres (square worlds for now).
    #[func]
    fn get_world_size(&self) -> f64 {
        lock(&self.shared).project.world.size_m[0]
    }

    #[func]
    fn set_world_size(&mut self, size_m: f64) -> bool {
        if !(size_m.is_finite() && size_m >= 1.0) {
            return self.fail("world size must be at least 1 m".into());
        }
        let _ = lock(&self.shared).edit("Set world size", Some("world.size"), |p| {
            p.world.size_m = [size_m, size_m];
            Ok(())
        });
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
        let _ = lock(&self.shared).edit("Set height range", Some("world.height"), |p| {
            p.world.height_range_m = [min_m, max_m];
            Ok(())
        });
        true
    }

    #[func]
    fn get_seed(&self) -> i64 {
        lock(&self.shared).project.world.seed as i64
    }

    #[func]
    fn set_seed(&mut self, seed: i64) {
        let _ = lock(&self.shared).edit("Set project seed", Some("world.seed"), |p| {
            p.world.seed = seed as u64;
            Ok(())
        });
    }

    // ---- export marks ---------------------------------------------------

    /// Outputs marked for export: one dictionary per output with node, port,
    /// label (the node type's label) and formats (PackedStringArray).
    #[func]
    fn get_exports(&self) -> VarArray {
        let s = lock(&self.shared);
        let mut grouped: Vec<(String, String, Vec<String>)> = Vec::new();
        for e in &s.project.exports {
            match grouped.iter_mut().find(|g| g.0 == e.node && g.1 == e.port) {
                Some(g) => g.2.push(e.format.clone()),
                None => grouped.push((e.node.clone(), e.port.clone(), vec![e.format.clone()])),
            }
        }
        let mut arr = VarArray::new();
        for (node, port, formats) in grouped {
            let label = s
                .project
                .graph
                .node(&node)
                .and_then(|n| registry().schema(&n.type_id))
                .map(|sc| sc.label.clone())
                .unwrap_or_else(|| "Unknown".into());
            let mut d = VarDictionary::new();
            put(&mut d, "node", GString::from(node.as_str()));
            put(&mut d, "port", GString::from(port.as_str()));
            put(&mut d, "label", GString::from(label.as_str()));
            put(
                &mut d,
                "formats",
                formats
                    .iter()
                    .map(|f| GString::from(f.as_str()))
                    .collect::<PackedStringArray>(),
            );
            arr.push(&d.to_variant());
        }
        arr
    }

    /// Mark or unmark `node.port` for export in `format` ("exr32" or "png16").
    #[func]
    fn set_export(&mut self, node: GString, port: GString, format: GString, on: bool) -> bool {
        let label = if on {
            "Mark for export"
        } else {
            "Unmark for export"
        };
        let result = lock(&self.shared).edit(label, None, |p| {
            p.set_export(&node.to_string(), &port.to_string(), &format.to_string(), on)
        });
        match result {
            Ok(()) => true,
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Formats `node.port` is marked for.
    #[func]
    fn get_export_formats(&self, node: GString, port: GString) -> PackedStringArray {
        lock(&self.shared)
            .project
            .export_formats(&node.to_string(), &port.to_string())
            .iter()
            .map(|f| GString::from(f.as_str()))
            .collect()
    }

    // ---- build settings -------------------------------------------------

    #[func]
    fn get_build_resolution(&self) -> i32 {
        lock(&self.shared).project.build.resolution as i32
    }

    #[func]
    fn set_build_resolution(&mut self, resolution: i32) {
        let mut s = lock(&self.shared);
        let r = resolution.clamp(2, terrain_core::grid::MAX_RESOLUTION as i32) as u32;
        if s.project.build.resolution != r {
            s.project.build.resolution = r;
            s.history.touch();
        }
    }

    #[func]
    fn get_build_folder(&self) -> GString {
        lock(&self.shared).project.build.folder.as_str().into()
    }

    #[func]
    fn set_build_folder(&mut self, folder: GString) {
        let mut s = lock(&self.shared);
        let f = folder.to_string();
        if s.project.build.folder != f {
            s.project.build.folder = f;
            s.history.touch();
        }
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

    fn replace(&mut self, project: Project, path: Option<PathBuf>, warnings: Vec<String>) {
        let mut s = lock(&self.shared);
        s.project = project;
        s.history.reset();
        s.path = path;
        self.warnings = warnings.iter().map(|w| GString::from(w.as_str())).collect();
    }

    /// A consistent copy of the project for a background job, plus the folder
    /// relative paths resolve against.
    pub(crate) fn snapshot(&self) -> (Project, Option<PathBuf>) {
        let s = lock(&self.shared);
        (s.project.clone(), s.base_dir())
    }
}
