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

pub mod adjust;
pub mod basic;
pub mod common;
pub mod data;
pub mod erosion;
pub mod gpu_check;
mod hydro;
pub mod kernels;
pub mod noise;
pub mod primitives;
pub mod terrain;
pub mod water;

use terrain_core::NodeRegistry;

/// Every built-in node type.
pub fn registry() -> NodeRegistry {
    let mut r = NodeRegistry::new();
    // Primitives
    r.register(basic::Constant::default());
    r.register(primitives::Gradient::default());
    r.register(primitives::Cone::default());
    r.register(primitives::Hemisphere::default());
    r.register(primitives::Shape::default());
    r.register(primitives::File::default());
    // Noise
    r.register(noise::Perlin::default());
    r.register(noise::Simplex::default());
    r.register(noise::ValueNoise::default());
    r.register(noise::Fbm::default());
    r.register(noise::Ridged::default());
    r.register(noise::Billow::default());
    r.register(noise::DomainWarp::default());
    r.register(noise::Voronoi::default());
    // Terrain
    r.register(terrain::Mountain::default());
    r.register(terrain::Ridge::default());
    r.register(terrain::Canyon::default());
    r.register(terrain::Crater::default());
    r.register(terrain::Plateau::default());
    r.register(terrain::Dunes::default());
    // Adjust
    r.register(basic::Levels::default());
    r.register(adjust::CurveNode::default());
    r.register(adjust::Clamp::default());
    r.register(adjust::Invert::default());
    r.register(adjust::Terrace::default());
    r.register(adjust::Blur::default());
    r.register(adjust::Sharpen::default());
    r.register(adjust::Transform::default());
    r.register(adjust::Warp::default());
    // Combine
    r.register(basic::Combine::default());
    // Data
    r.register(data::HeightMask::default());
    r.register(data::Slope::default());
    r.register(data::Curvature::default());
    r.register(data::Aspect::default());
    r.register(data::SelectRange::default());
    r.register(data::Distance::default());
    r.register(water::Flow::default());
    r.register(erosion::RockHardness::default());
    // Simulate
    r.register(erosion::Hydraulic::default());
    r.register(erosion::Thermal::default());
    r
}
