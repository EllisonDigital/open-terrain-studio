//! Times the v0.1 exit-criteria graph (fBm -> Levels) at common resolutions.
//! Run: cargo run --release -p terrain-nodes --example bench_preview
use std::time::Instant;

use terrain_core::{EvalOptions, GridSpec, Project, evaluate_node};

fn main() {
    let reg = terrain_nodes::registry();
    let mut p = Project::default();
    let fbm = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let levels = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &fbm, "out", &levels, "in").unwrap();
    println!("threads: {}", rayon_threads());
    for res in [512u32, 1024, 2048, 4096] {
        let spec = GridSpec::full_world(&p.world, res).unwrap();
        let t = Instant::now();
        evaluate_node(&p.graph, &reg, &p.world, spec, &levels, &EvalOptions::default()).unwrap();
        println!(
            "{res:>5}²  fBm(6 octaves) -> Levels: {:>7.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}
