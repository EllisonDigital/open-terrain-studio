//! Renders a node to a shaded PNG, for checking how nodes look without the app.
//!
//! ```sh
//! # One node type with default settings (inputs fed from a smooth fBm):
//! cargo run --release -p terrain-nodes --example render -- terrain.mountain out.png [resolution]
//! # A node in a project (the viewed node if none is given):
//! cargo run --release -p terrain-nodes --example render -- project.otstudio out.png [resolution] [node]
//! ```
//!
//! Heightfields are hillshaded with a height tint; masks are drawn in grey.

use std::path::Path;

use terrain_core::{EvalOptions, Grid, GridSpec, Project, Value, evaluate_node};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render <type_id | project.otstudio> <out.png> [resolution] [node]");
        std::process::exit(2);
    }
    let reg = terrain_nodes::registry();
    let res: u32 = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(1025);
    let (project, node, base_dir) = if args[0].ends_with(".otstudio") {
        let path = Path::new(&args[0]);
        let (p, warnings) = Project::load(path, &reg).expect("load project");
        for w in warnings {
            eprintln!("warning: {w}");
        }
        let node = args.get(3).cloned().unwrap_or_else(|| {
            p.ui["viewed_node"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| p.graph.nodes().last().unwrap().id.clone())
        });
        (p, node, path.parent().map(Path::to_path_buf))
    } else {
        let mut p = Project::default();
        let id = p.graph.add_node(&reg, &args[0], [0.0, 0.0]).expect("node type");
        let schema = reg.schema(&args[0]).unwrap();
        for input in schema.inputs.iter().filter(|i| !i.optional) {
            let src = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
            p.graph
                .set_param(
                    &reg,
                    &src,
                    "feature_size_m",
                    terrain_core::ParamValue::Float(2500.0),
                )
                .unwrap();
            p.graph.connect(&reg, &src, "out", &id, &input.key).unwrap();
        }
        (p, id, None)
    };
    let spec = GridSpec::full_world(&project.world, res).unwrap();
    let t = std::time::Instant::now();
    let out = evaluate_node(
        &project.graph,
        &reg,
        &project.world,
        spec,
        &node,
        &EvalOptions {
            base_dir: base_dir.as_deref(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let value = out.values().next().unwrap();
    let (lo, hi) = value.grid().min_max();
    println!("{node} at {res}²: {lo:.2} .. {hi:.2}  ({ms:.0} ms)");
    let rgb = match value {
        Value::Heightfield(g) => shade(g, project.world.height_range_m),
        Value::Mask(g) => g
            .data
            .iter()
            .flat_map(|&v| [(v.clamp(0.0, 1.0) * 255.0) as u8; 3])
            .collect(),
    };
    image::RgbImage::from_raw(res, res, rgb)
        .unwrap()
        .save(&args[1])
        .unwrap();
}

/// Hillshade (sun from the upper left) times a height tint.
fn shade(g: &Grid, range: [f32; 2]) -> Vec<u8> {
    let (gx, gy) = terrain_core::ops::gradient(g);
    let (lx, ly, lz) = (-0.5f32, -0.6f32, 0.62f32);
    let mut out = Vec::with_capacity(g.data.len() * 3);
    for idx in 0..g.data.len() {
        let (nx, ny, nz) = (-gx.data[idx], -gy.data[idx], 1.0f32);
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let light = ((nx * lx + ny * ly + nz * lz) / len).max(0.0) * 0.85 + 0.15;
        let t = ((g.data[idx] - range[0]) / (range[1] - range[0])).clamp(0.0, 1.0);
        let tint = [0.45 + 0.4 * t, 0.42 + 0.35 * t, 0.36 + 0.4 * t];
        for c in tint {
            out.push((c * light * 255.0).clamp(0.0, 255.0) as u8);
        }
    }
    out
}
