//! Shared test helpers: build a small project around any node type.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use terrain_core::{
    EvalOptions, Grid, GridSpec, NodeRegistry, ParamValue, PortType, Project, Value, evaluate_node,
};

pub fn registry() -> NodeRegistry {
    terrain_nodes::registry()
}

/// A folder under the system temp dir, unique to this test process.
pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ots-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 16-bit PNG heightmap (smooth bumps) for the File node.
pub fn test_png(dir: &Path) -> PathBuf {
    let path = dir.join("heightmap.png");
    if !path.exists() {
        let n = 257u32;
        let img = image::ImageBuffer::<image::Luma<u16>, _>::from_fn(n, n, |i, j| {
            let (x, y) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
            let v = 0.5 + 0.25 * libm::sin(x * 7.0) * libm::cos(y * 5.0) + 0.2 * x;
            image::Luma([(v.clamp(0.0, 1.0) * 65535.0) as u16])
        });
        img.save(&path).unwrap();
    }
    path
}

/// A project containing one node of `type_id`, every required input fed by a
/// smooth fBm (3 octaves, 2 km features, spanning the world height range) so
/// neighbourhood operations are resolvable at modest resolutions. Returns
/// the project and the node id.
pub fn project_for(reg: &NodeRegistry, type_id: &str, files: &Path) -> (Project, String) {
    let mut p = Project::default();
    let id = p.graph.add_node(reg, type_id, [0.0, 0.0]).unwrap();
    let schema = reg.schema(type_id).unwrap().clone();
    for input in schema.inputs.iter().filter(|i| !i.optional) {
        let src = smooth_source(reg, &mut p);
        p.graph.connect(reg, &src, "out", &id, &input.key).unwrap();
    }
    // Simulations: a short run keeps the all-node checks fast; tests/erosion*.rs
    // cover full-length erosion.
    for (key, value) in [("duration_s", 6.0), ("duration_kyr", 100.0), ("detail_m", 32.0)] {
        if schema.param(key).is_some() {
            p.graph
                .set_param(reg, &id, key, ParamValue::Float(value))
                .unwrap();
        }
    }
    if type_id == "primitive.file" {
        let png = test_png(files);
        p.graph
            .set_param(
                reg,
                &id,
                "path",
                ParamValue::Text(png.to_string_lossy().into_owned()),
            )
            .unwrap();
    }
    (p, id)
}

/// Add the smooth fBm used to feed inputs; returns its id.
pub fn smooth_source(reg: &NodeRegistry, p: &mut Project) -> String {
    let src = p.graph.add_node(reg, "noise.fbm", [0.0, 0.0]).unwrap();
    for (k, v) in [
        ("feature_size_m", ParamValue::Float(2000.0)),
        ("octaves", ParamValue::Int(3)),
        ("height_m", ParamValue::Float(2000.0)),
    ] {
        p.graph.set_param(reg, &src, k, v).unwrap();
    }
    src
}

/// Evaluate `node` over the whole world; returns its first declared output.
pub fn eval(p: &Project, reg: &NodeRegistry, node: &str, res: u32) -> (Arc<Grid>, PortType) {
    eval_with(p, reg, node, res, &EvalOptions::default())
}

pub fn eval_with(
    p: &Project,
    reg: &NodeRegistry,
    node: &str,
    res: u32,
    opts: &EvalOptions,
) -> (Arc<Grid>, PortType) {
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let out =
        evaluate_node(&p.graph, reg, &p.world, spec, node, opts).unwrap_or_else(|e| panic!("{node}: {e}"));
    match out.values().next().unwrap() {
        Value::Heightfield(g) => (g.clone(), PortType::Heightfield),
        Value::Mask(g) => (g.clone(), PortType::Mask),
    }
}

/// Bits of every sample, for exact comparisons.
pub fn bits(g: &Grid) -> Vec<u32> {
    g.data.iter().map(|v| v.to_bits()).collect()
}
