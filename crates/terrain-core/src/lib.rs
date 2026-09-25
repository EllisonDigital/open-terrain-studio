//! # terrain-core
//!
//! The OpenTerrainStudio terrain engine. This crate has **no Godot dependency**:
//! it owns the world definition, heightfield data, the node graph, evaluation,
//! project files and export writers. The Godot app is only a view onto it.
//!
//! See `docs/ARCHITECTURE.md` for the design this crate implements.

pub mod cache;
pub mod error;
pub mod eval;
pub mod export;
pub mod graph;
pub mod grid;
pub mod history;
pub mod import;
pub mod node;
pub mod ops;
pub mod params;
pub mod project;
pub mod seed;
pub mod world;

pub use cache::{CacheStats, EvalCache};
pub use error::CoreError;
pub use eval::{EvalOptions, evaluate_node};
pub use graph::{Graph, Link, NodeId, NodeInstance};
pub use grid::{Grid, GridSpec};
pub use history::{EditState, History};
pub use node::{EvalContext, Field, NodeKind, NodeRegistry, NodeSchema, Outputs, PortDef, PortType, Value};
pub use params::{Curve, ParamDef, ParamKind, ParamValue};
pub use project::Project;
pub use world::World;

/// Version of the application/engine, written into project and build files.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
