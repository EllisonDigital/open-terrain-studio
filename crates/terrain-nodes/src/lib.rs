//! # terrain-nodes
//!
//! The OpenTerrainStudio node library. Each node is one type implementing
//! [`terrain_core::NodeKind`]; [`registry`] lists them all.
//!
//! ## Adding a node
//!
//! 1. Write a struct with a `NodeSchema` (ports + parameters, sizes in metres).
//! 2. Implement `NodeKind::evaluate` using world positions from `ctx.spec`.
//! 3. Register it in [`registry`] and add a test. The editor UI is generated
//!    from the schema, so no Godot code is needed.

pub mod basic;
pub mod noise;

use terrain_core::NodeRegistry;

/// Every built-in node type.
pub fn registry() -> NodeRegistry {
    let mut r = NodeRegistry::new();
    r.register(basic::Constant::default());
    r.register(noise::Perlin::default());
    r.register(noise::Simplex::default());
    r.register(noise::Fbm::default());
    r.register(basic::Combine::default());
    r.register(basic::Levels::default());
    r
}
