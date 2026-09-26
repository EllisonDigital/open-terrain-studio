//! v0.8 exit-criteria build: the River coast example (erosion, rivers,
//! lakes, sea, snow, colour, splat weights, normals) with three chained
//! populations (trees, shrubs, grass) added, every output marked, built at
//! any resolution. Builds over 4,097 are computed in tiles.
//!
//! cargo run --release -p terrain-nodes --example bench_build -- 8193 out/8k [tile-size] [file-tile-size]
//!
//! Prints the time taken and cache counters; measure peak memory from outside
//! (e.g. PowerShell's PeakWorkingSet64, or /usr/bin/time -v).
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use terrain_core::export::build_marked;
use terrain_core::project::FileTiles;
use terrain_core::{EvalCache, EvalOptions, ParamValue, Project};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resolution: u32 = args
        .first()
        .map(|v| v.parse().expect("resolution"))
        .unwrap_or(8193);
    let folder = args.get(1).cloned().unwrap_or_else(|| "bench_build".into());
    let reg = terrain_nodes::registry();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/examples/river_coast.otstudio");
    let (mut p, warnings) = Project::load(&example, &reg).expect("load River coast");
    assert!(warnings.is_empty(), "{warnings:?}");
    if let Some(t) = args.get(2) {
        p.build.tile_size = t.parse().expect("tile size");
    }
    if let Some(t) = args.get(3) {
        p.build.file_tiles = Some(FileTiles {
            size: t.parse().expect("file tile size"),
            pattern: "{name}_x{x}_y{y}".into(),
        });
    }

    // Three populations on the snowy terrain, watered by the rivers, each
    // avoiding the one before.
    let (terrain, rivers, snow) = (("n_0006", "height"), ("n_0003", "river"), ("n_0006", "snow"));
    let mut previous: Option<String> = None;
    for (k, kind) in ["vegetation.trees", "vegetation.shrubs", "vegetation.grass"]
        .iter()
        .enumerate()
    {
        let id = p.graph.add_node(&reg, kind, [1200.0, 400.0 * k as f32]).unwrap();
        p.graph.connect(&reg, terrain.0, terrain.1, &id, "in").unwrap();
        p.graph.connect(&reg, rivers.0, rivers.1, &id, "water").unwrap();
        p.graph.connect(&reg, snow.0, snow.1, &id, "snow").unwrap();
        if let Some(prev) = &previous {
            p.graph.connect(&reg, prev, "occupied", &id, "occupied").unwrap();
        }
        if *kind == "vegetation.grass" {
            // Sparser than the default, to keep the point file manageable.
            p.graph
                .set_param(&reg, &id, "spacing_m", ParamValue::Float(4.0))
                .unwrap();
        }
        p.set_export(&id, "density", "png16", true).unwrap();
        p.set_export(&id, "points", "csv", true).unwrap();
        previous = Some(id);
    }
    let marked = p.exports.len();

    let cache = EvalCache::default();
    let last = AtomicU32::new(0);
    let progress = |f: f32| {
        let pct = (f * 100.0) as u32;
        if pct >= last.load(Ordering::Relaxed) + 5 {
            last.store(pct, Ordering::Relaxed);
            eprintln!("  {pct}%");
        }
    };
    let start = Instant::now();
    let written = build_marked(
        &p,
        &reg,
        resolution,
        Path::new(&folder),
        &EvalOptions {
            cache: Some(&cache),
            progress: Some(&progress),
            ..EvalOptions::default()
        },
    )
    .expect("build");
    let s = cache.stats();
    println!(
        "{resolution}²: {marked} marked outputs, {} files in {:.1} s (tiles of {}); cache: {} computed, {} reused",
        written.len(),
        start.elapsed().as_secs_f64(),
        p.build.tile_size,
        s.misses,
        s.hits
    );
}
