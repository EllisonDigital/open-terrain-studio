//! CPU milestone benchmark; fixed eight-worker pool by default.
//! cargo run --release -p terrain-nodes --example bench_erosion -- 1024 8 1000
//! Arguments: resolution, workers, hydraulic duration in kyr (thermal runs 60 s).
//! Append an output folder to write the project and all erosion maps.
//!
//! The world is the default 8,192 m, so at 1,024² the hydraulic simulation
//! runs at its full default detail (8 m cells, a 1,025² simulation grid): the
//! slowest case for that output resolution.
use std::path::Path;
use std::time::Instant;
use terrain_core::export::{write_exr32, write_png16};
use terrain_core::{EvalOptions, GridSpec, ParamValue, Project, evaluate_node};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resolution: u32 = args
        .first()
        .map(|v| v.parse().expect("resolution"))
        .unwrap_or(1024);
    let threads: usize = args.get(1).map(|v| v.parse().expect("threads")).unwrap_or(8);
    let duration_kyr: f64 = args
        .get(2)
        .map(|v| v.parse().expect("duration"))
        .unwrap_or(1000.0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        let registry = terrain_nodes::registry();
        let mut project = Project::default();
        project.world.size_m = [8192.0, 8192.0];
        let source = project.graph.add_node(&registry, "noise.fbm", [0.0, 0.0]).unwrap();
        project.graph.set_param(&registry, &source, "feature_size_m", ParamValue::Float(1400.0)).unwrap();
        project.graph.set_param(&registry, &source, "height_m", ParamValue::Float(1600.0)).unwrap();
        let spec = GridSpec::full_world(&project.world, resolution).unwrap();
        let before = evaluate_node(&project.graph, &registry, &project.world, spec, &source, &EvalOptions::default()).unwrap();
        for kind in ["simulate.hydraulic", "simulate.thermal"] {
            let node = project.graph.add_node(&registry, kind, [350.0, if kind.ends_with("thermal") {300.0} else {0.0}]).unwrap();
            project.graph.connect(&registry, &source, "out", &node, "in").unwrap();
            let (key, value, unit) = if kind.ends_with("hydraulic") {
                ("duration_kyr", duration_kyr, "kyr")
            } else {
                ("duration_s", 60.0, "s")
            };
            project.graph.set_param(&registry, &node, key, ParamValue::Float(value)).unwrap();
            let start = Instant::now();
            let outputs = evaluate_node(&project.graph, &registry, &project.world, spec, &node, &EvalOptions::default()).unwrap();
            let elapsed = start.elapsed().as_secs_f64();
            let mean_change: f64 = outputs["height"].grid().data.iter().zip(&before["out"].grid().data).map(|(a,b)| (a-b).abs() as f64).sum::<f64>() / spec.len() as f64;
            println!("{kind}: {resolution}², {threads} workers, {value} {unit}: {elapsed:.3}s; mean |height change| {mean_change:.4}m");
            if let Some(folder) = args.get(3) {
                let folder = Path::new(folder); std::fs::create_dir_all(folder).unwrap();
                for (port,value) in outputs {
                    let stem = format!("{}-{port}", kind.replace('.', "-"));
                    write_exr32(value.grid(), &folder.join(format!("{stem}.exr"))).unwrap();
                    let png = if port == "height" { value.grid().map(|v| project.world.normalise(v)) } else { value.grid().as_ref().clone() };
                    write_png16(&png, &folder.join(format!("{stem}.png"))).unwrap();
                }
                write_png16(&before["out"].grid().map(|v| project.world.normalise(v)), &folder.join("before.png")).unwrap();
                std::fs::write(folder.join("erosion.otstudio"), project.to_json().unwrap()).unwrap();
            }
        }
    });
}
