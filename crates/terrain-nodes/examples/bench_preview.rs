//! Times preview evaluation.
//! Run: cargo run --release -p terrain-nodes --example bench_preview
//!
//! 1. The v0.1 exit-criteria graph (fBm -> Levels) at common resolutions.
//! 2. The alpine range example at 1024²: cold, then after editing its last
//!    node (only that node recomputes; everything upstream comes from the cache).
use std::time::Instant;

use terrain_core::{EvalCache, EvalOptions, GridSpec, ParamValue, Project, evaluate_node};

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

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/examples/alpine_range.otstudio");
    let (mut alpine, _) = Project::load(&path, &reg).unwrap();
    let viewed = alpine.ui["viewed_node"].as_str().unwrap().to_string();
    let spec = GridSpec::full_world(&alpine.world, 1024).unwrap();
    let cache = EvalCache::default();
    let opts = EvalOptions {
        cache: Some(&cache),
        ..Default::default()
    };
    let run = |p: &Project, what: &str| {
        let before = cache.stats().misses;
        let t = Instant::now();
        evaluate_node(&p.graph, &reg, &p.world, spec, &viewed, &opts).unwrap();
        println!(
            " 1024²  alpine example, {what:<28} {:>7.1} ms  ({} of {} nodes computed)",
            t.elapsed().as_secs_f64() * 1000.0,
            cache.stats().misses - before,
            p.graph.evaluation_order(&viewed).unwrap().len()
        );
    };
    run(&alpine, "cold:");
    alpine
        .graph
        .set_param(&reg, &viewed, "gamma", ParamValue::Float(1.3))
        .unwrap();
    run(&alpine, "after editing the last node:");
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}
