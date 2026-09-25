//! # terrain-godot
//!
//! The GDExtension that exposes `terrain-core` to the Godot app. Deliberately
//! thin (ARCHITECTURE.md §3): GDScript sees five classes and never computes
//! terrain itself.
//!
//! | Class             | Kind       | Job                                              |
//! |-------------------|------------|--------------------------------------------------|
//! | `TerrainProject`  | RefCounted | load/save, world, undo/redo, export marks, UI    |
//! | `TerrainGraph`    | RefCounted | add/remove/connect nodes, parameters, schemas    |
//! | `TerrainBuilder`  | Node       | evaluates a node on a worker thread (preview)    |
//! | `TerrainPreview`  | RefCounted | one evaluated result, as a Godot `Image`         |
//! | `TerrainExporter` | Node       | exports / builds EXR/PNG + build.json on a worker |
//!
//! Preview and build jobs share one [`cache`], so viewing a node after editing
//! a downstream one, or building outputs that share upstream nodes, reuses
//! earlier results.

mod builder;
mod convert;
mod exporter;
mod graph;
mod jobs;
mod preview;
mod project;

use std::sync::OnceLock;

use godot::prelude::*;
use terrain_core::{EvalCache, NodeRegistry};

struct OpenTerrainStudio;

#[gdextension]
unsafe impl ExtensionLibrary for OpenTerrainStudio {}

/// The node registry, built once.
pub(crate) fn registry() -> &'static NodeRegistry {
    static REGISTRY: OnceLock<NodeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(terrain_nodes::registry)
}

/// Node results shared by every preview and build job.
pub(crate) fn cache() -> &'static EvalCache {
    static CACHE: OnceLock<EvalCache> = OnceLock::new();
    CACHE.get_or_init(EvalCache::default)
}
