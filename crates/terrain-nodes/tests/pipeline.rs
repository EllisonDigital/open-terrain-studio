//! End-to-end tests: graph editing, evaluation, caching, parameter ports,
//! determinism, resolution independence, project files and export.

mod support;

use terrain_core::export::{ExportFormat, ExportRequest, build_marked, export_node};
use terrain_core::{
    CoreError, EvalCache, EvalOptions, GridSpec, NodeRegistry, ParamValue, Project, Value, evaluate_node,
};

use support::{registry, temp_dir};

/// fBm -> Levels, the v0.1 exit-criteria graph.
fn sample_project(reg: &NodeRegistry) -> (Project, String, String) {
    let mut p = Project::default();
    let fbm = p.graph.add_node(reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let levels = p.graph.add_node(reg, "adjust.levels", [300.0, 0.0]).unwrap();
    p.graph.connect(reg, &fbm, "out", &levels, "in").unwrap();
    p.graph
        .set_param(reg, &fbm, "feature_size_m", ParamValue::Int(900))
        .unwrap();
    (p, fbm, levels)
}

fn eval(p: &Project, reg: &NodeRegistry, node: &str, res: u32) -> std::sync::Arc<terrain_core::Grid> {
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let out = evaluate_node(&p.graph, reg, &p.world, spec, node, &EvalOptions::default()).unwrap();
    match out.get("out").unwrap() {
        Value::Heightfield(g) | Value::Mask(g) => g.clone(),
    }
}

#[test]
fn every_node_type_evaluates_with_defaults() {
    let reg = registry();
    let dir = temp_dir("defaults");
    for schema in reg.schemas() {
        let (p, id) = support::project_for(&reg, &schema.type_id, &dir);
        let spec = GridSpec::full_world(&p.world, 64).unwrap();
        let out = evaluate_node(&p.graph, &reg, &p.world, spec, &id, &EvalOptions::default())
            .unwrap_or_else(|e| panic!("{} failed: {e}", schema.type_id));
        for port in &schema.outputs {
            let v = out
                .get(&port.key)
                .unwrap_or_else(|| panic!("{} missing {}", schema.type_id, port.key));
            assert_eq!(v.port_type(), port.ty, "{} output type", schema.type_id);
        }
    }
}

#[test]
fn fbm_levels_fills_output_range() {
    let reg = registry();
    let (p, _, levels) = sample_project(&reg);
    let g = eval(&p, &reg, &levels, 256);
    let (lo, hi) = g.min_max();
    assert!(lo.abs() < 1e-3 && (hi - 2000.0).abs() < 1e-2, "{lo} {hi}");
}

#[test]
fn deterministic_across_thread_counts() {
    let reg = registry();
    let (p, _, levels) = sample_project(&reg);
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
    let a = one.install(|| eval(&p, &reg, &levels, 257));
    let b = many.install(|| eval(&p, &reg, &levels, 257));
    let bits = |g: &terrain_core::Grid| g.data.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
}

#[test]
fn resolution_independent() {
    // The same world position must give (nearly) the same height at 256² and 1024².
    let reg = registry();
    let (p, fbm, _) = sample_project(&reg);
    let low = eval(&p, &reg, &fbm, 256);
    let high = eval(&p, &reg, &fbm, 1024);
    let mut worst = 0.0f32;
    for j in 0..low.spec.height {
        for i in 0..low.spec.width {
            let x = low.spec.x_m(i);
            let y = low.spec.y_m(j);
            worst = worst.max((low.get(i, j) - high.sample_bilinear_m(x, y)).abs());
        }
    }
    // Height 1000 m; allow 1% for bilinear interpolation of the finest octaves.
    assert!(worst < 10.0, "max difference {worst} m");
}

#[test]
fn seed_changes_result_and_node_id_is_stable() {
    let reg = registry();
    let (mut p, fbm, _) = sample_project(&reg);
    let a = eval(&p, &reg, &fbm, 64);
    p.graph.set_param(&reg, &fbm, "seed", ParamValue::Int(7)).unwrap();
    let b = eval(&p, &reg, &fbm, 64);
    assert_ne!(a.data, b.data);
    p.world.seed += 1;
    let c = eval(&p, &reg, &fbm, 64);
    assert_ne!(b.data, c.data);
}

#[test]
fn graph_rejects_cycles_and_missing_inputs() {
    let reg = registry();
    let mut p = Project::default();
    let a = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    let b = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &a, "out", &b, "in").unwrap();
    assert!(matches!(
        p.graph.connect(&reg, &b, "out", &a, "in"),
        Err(CoreError::Cycle)
    ));
    assert!(matches!(
        p.graph.connect(&reg, &a, "out", &a, "in"),
        Err(CoreError::Cycle)
    ));
    assert!(p.graph.connect(&reg, &a, "nope", &b, "in").is_err());

    let spec = GridSpec::full_world(&p.world, 16).unwrap();
    let err = evaluate_node(&p.graph, &reg, &p.world, spec, &b, &EvalOptions::default()).unwrap_err();
    assert!(matches!(err, CoreError::MissingInput { .. }), "{err}");
}

#[test]
fn connecting_replaces_existing_input_link() {
    let reg = registry();
    let mut p = Project::default();
    let n1 = p.graph.add_node(&reg, "noise.perlin", [0.0, 0.0]).unwrap();
    let n2 = p.graph.add_node(&reg, "noise.simplex", [0.0, 0.0]).unwrap();
    let lv = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &n1, "out", &lv, "in").unwrap();
    p.graph.connect(&reg, &n2, "out", &lv, "in").unwrap();
    assert_eq!(p.graph.links().len(), 1);
    assert_eq!(p.graph.link_into(&lv, "in").unwrap().from.0, n2);
    p.graph.remove_node(&n2).unwrap();
    assert!(p.graph.links().is_empty());
}

#[test]
fn project_roundtrip_and_ids_never_reused() {
    let reg = registry();
    let (mut p, fbm, levels) = sample_project(&reg);
    p.graph.remove_node(&levels).unwrap();
    let json = p.to_json().unwrap();
    let (loaded, warnings) = Project::from_json(&json, &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(loaded.graph, p.graph);
    assert_eq!(loaded.world, p.world);

    let mut loaded = loaded;
    let new_id = loaded.graph.add_node(&reg, "noise.perlin", [0.0, 0.0]).unwrap();
    assert_ne!(new_id, levels, "a deleted node's id was reused");
    assert_ne!(new_id, fbm);
}

#[test]
fn loads_the_documented_example_and_keeps_unknown_nodes() {
    // The example from ARCHITECTURE.md §10 (erosion doesn't exist yet in v0.1).
    let json = r#"{
      "format_version": 1,
      "app_version": "0.1.0",
      "world": { "size_m": [4096, 4096], "height_range_m": [0, 2400], "seed": 12345 },
      "nodes": [
        { "id": "n_7f3a", "type": "noise.perlin", "type_version": 1,
          "pos": [120, 80], "params": { "feature_size_m": 800, "octaves": 6 } },
        { "id": "n_91c2", "type": "simulate.erosion", "type_version": 1,
          "pos": [420, 80], "params": { "duration": 0.6, "rock_softness": 0.4 } }
      ],
      "links": [ { "from": ["n_7f3a", "out"], "to": ["n_91c2", "in"] } ],
      "exports": [ { "node": "n_91c2", "port": "height", "format": "exr32" } ],
      "build": { "resolution": 4096, "folder": "output/" },
      "ui": { "viewed_node": "n_91c2", "camera": {} }
    }"#;
    let reg = registry();
    let (p, warnings) = Project::from_json(json, &reg).unwrap();
    assert_eq!(p.graph.nodes().count(), 2);
    assert_eq!(p.graph.links().len(), 1, "link to placeholder node was dropped");
    assert!(warnings.iter().any(|w| w.contains("simulate.erosion")));
    // Perlin has no "octaves" parameter: dropped. feature_size_m kept as a float.
    let perlin = p.graph.node("n_7f3a").unwrap();
    assert_eq!(
        perlin.params.get("feature_size_m"),
        Some(&ParamValue::Float(800.0))
    );
    assert!(!perlin.params.contains_key("octaves"));
    // The unknown node's parameters survive a save.
    let saved = p.to_json().unwrap();
    assert!(saved.contains("rock_softness"));
    // New ids continue after the highest loaded id.
    let mut p = p;
    assert_eq!(
        p.graph.add_node(&reg, "noise.perlin", [0.0, 0.0]).unwrap(),
        "n_91c3"
    );
}

#[test]
fn rejects_newer_format() {
    let json = r#"{"format_version": 99, "app_version": "9", "world": {"size_m":[1,1],"height_range_m":[0,1],"seed":0}}"#;
    assert!(Project::from_json(json, &registry()).is_err());
}

#[test]
fn export_writes_valid_deterministic_files() {
    let reg = registry();
    let (p, _, levels) = sample_project(&reg);
    let dir = std::env::temp_dir().join(format!("ots-export-test-{}", std::process::id()));
    let run = |sub: &str| {
        let folder = dir.join(sub);
        export_node(
            &p,
            &reg,
            &ExportRequest {
                node: &levels,
                port: "out",
                resolution: 129,
                folder: &folder,
                formats: &[ExportFormat::Exr32, ExportFormat::Png16],
            },
            &EvalOptions::default(),
        )
        .unwrap()
    };
    let a = run("a");
    let b = run("b");
    assert_eq!(a.len(), 3); // exr, png, build.json
    for (fa, fb) in a.iter().zip(&b) {
        if fa.file_name().unwrap() == "build.json" {
            continue;
        }
        assert_eq!(
            std::fs::read(fa).unwrap(),
            std::fs::read(fb).unwrap(),
            "{fa:?} not byte-identical"
        );
    }

    // The EXR holds heights in metres.
    let exr = a.iter().find(|p| p.extension().unwrap() == "exr").unwrap();
    let img = exr::prelude::read_first_flat_layer_from_file(exr).unwrap();
    assert_eq!(img.layer_data.size.width(), 129);
    let values = img.layer_data.channel_data.list[0]
        .sample_data
        .values_as_f32()
        .collect::<Vec<_>>();
    let expected = eval(&p, &reg, &levels, 129);
    assert_eq!(values, expected.data);

    // The PNG is 16-bit and spans the full range (Levels maps to the world range).
    let png = a.iter().find(|p| p.extension().unwrap() == "png").unwrap();
    let img = image::open(png).unwrap().into_luma16();
    assert_eq!(img.dimensions(), (129, 129));
    let max = *img.as_raw().iter().max().unwrap();
    let min = *img.as_raw().iter().min().unwrap();
    assert_eq!((min, max), (0, 65535));

    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("a/build.json")).unwrap()).unwrap();
    assert_eq!(info["resolution"][0], 129);
    assert_eq!(info["cell_size_m"][0], 64.0);
    assert_eq!(info["files"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

/// fBm -> Levels -> Blur, evaluated through a cache.
fn chain(reg: &NodeRegistry) -> (Project, [String; 3]) {
    let (mut p, fbm, levels) = sample_project(reg);
    let blur = p.graph.add_node(reg, "adjust.blur", [600.0, 0.0]).unwrap();
    p.graph.connect(reg, &levels, "out", &blur, "in").unwrap();
    (p, [fbm, levels, blur])
}

fn eval_cached(p: &Project, reg: &NodeRegistry, cache: &EvalCache, node: &str, res: u32) -> Vec<f32> {
    support::eval_with(
        p,
        reg,
        node,
        res,
        &EvalOptions {
            cache: Some(cache),
            ..Default::default()
        },
    )
    .0
    .data
    .clone()
}

#[test]
fn editing_downstream_never_recomputes_upstream() {
    let reg = registry();
    let (mut p, [fbm, levels, blur]) = chain(&reg);
    let cache = EvalCache::default();
    let first = eval_cached(&p, &reg, &cache, &blur, 129);
    assert_eq!(cache.stats().misses, 3);

    // Same graph again: nothing recomputes, and the result is identical.
    assert_eq!(eval_cached(&p, &reg, &cache, &blur, 129), first);
    assert_eq!(cache.stats().misses, 3);
    assert_eq!(cache.stats().hits, 3);

    // Edit the last node: only it recomputes.
    p.graph
        .set_param(&reg, &blur, "radius_m", ParamValue::Float(300.0))
        .unwrap();
    eval_cached(&p, &reg, &cache, &blur, 129);
    assert_eq!(cache.stats().misses, 4);

    // Edit the middle node: it and the blur recompute, the fBm doesn't.
    p.graph
        .set_param(&reg, &levels, "gamma", ParamValue::Float(1.5))
        .unwrap();
    eval_cached(&p, &reg, &cache, &blur, 129);
    assert_eq!(cache.stats().misses, 6);

    // Undoing an edit (setting the old value back) hits the cache again.
    p.graph
        .set_param(&reg, &levels, "gamma", ParamValue::Float(1.0))
        .unwrap();
    eval_cached(&p, &reg, &cache, &blur, 129);
    assert_eq!(cache.stats().misses, 6);

    // A different resolution is a different entry; so is a world change.
    eval_cached(&p, &reg, &cache, &fbm, 65);
    assert_eq!(cache.stats().misses, 7);
    p.world.height_range_m = [0.0, 3000.0];
    eval_cached(&p, &reg, &cache, &fbm, 65);
    assert_eq!(cache.stats().misses, 8);
}

#[test]
fn cached_results_match_uncached_and_cache_stays_within_budget() {
    let reg = registry();
    let (p, [_, _, blur]) = chain(&reg);
    let plain = support::eval(&p, &reg, &blur, 129).0;
    // Room for about two 129² grids.
    let cache = EvalCache::new(129 * 129 * 4 * 2 + 16);
    for _ in 0..3 {
        assert_eq!(eval_cached(&p, &reg, &cache, &blur, 129), plain.data);
    }
    let stats = cache.stats();
    assert!(stats.entries <= 2 && stats.evictions > 0, "{stats:?}");
}

#[test]
fn masks_drive_parameters_through_ports() {
    let reg = registry();
    let mut p = Project::default();
    let cone = p.graph.add_node(&reg, "primitive.cone", [0.0, 0.0]).unwrap();
    let grad = p.graph.add_node(&reg, "primitive.gradient", [0.0, 0.0]).unwrap();
    // A port only exists once the parameter is exposed.
    assert!(p.graph.connect(&reg, &grad, "out", &cone, "p:height_m").is_err());
    p.graph.set_exposed(&reg, &cone, "height_m", true).unwrap();
    assert!(
        p.graph.set_exposed(&reg, &cone, "radius_m", true).is_err(),
        "radius is not drivable"
    );
    let ports = p.graph.input_ports(&reg, &cone).unwrap();
    assert_eq!(ports.len(), 1);
    assert_eq!(
        (ports[0].key.as_str(), ports[0].param.as_str()),
        ("p:height_m", "height_m")
    );

    let undriven = support::eval(&p, &reg, &cone, 65).0;
    p.graph.connect(&reg, &grad, "out", &cone, "p:height_m").unwrap();
    let driven = support::eval(&p, &reg, &cone, 65).0;
    // The gradient (0 at the left edge .. 1 at the right, as a mask of the
    // 0..2000 m world with height 1000) scales the cone's 1500 m height.
    let centre = undriven.get(32, 32);
    assert!((centre - 1500.0).abs() < 1.0);
    let mask_at_centre = 0.25; // gradient 500 m at the centre -> 0.25 of 2000 m
    assert!(
        (driven.get(32, 32) - centre * mask_at_centre).abs() < 1.0,
        "{}",
        driven.get(32, 32)
    );

    // Saved and loaded with the project.
    let (loaded, warnings) = Project::from_json(&p.to_json().unwrap(), &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(loaded.graph, p.graph);

    // Hiding the port removes its link.
    p.graph.set_exposed(&reg, &cone, "height_m", false).unwrap();
    assert!(p.graph.links().is_empty());
    assert_eq!(support::eval(&p, &reg, &cone, 65).0.data, undriven.data);
}

#[test]
fn build_writes_every_marked_output_once() {
    let reg = registry();
    let (mut p, [fbm, levels, blur]) = chain(&reg);
    let slope = p.graph.add_node(&reg, "data.slope", [600.0, 200.0]).unwrap();
    p.graph.connect(&reg, &levels, "out", &slope, "in").unwrap();
    let folder = temp_dir("build");
    let opts = EvalOptions {
        base_dir: Some(&folder),
        ..Default::default()
    };
    assert!(
        build_marked(&p, &reg, 65, std::path::Path::new("out"), &opts).is_err(),
        "nothing marked"
    );

    p.set_export(&blur, "out", "exr32", true).unwrap();
    p.set_export(&blur, "out", "png16", true).unwrap();
    p.set_export(&slope, "out", "png16", true).unwrap();
    assert!(p.set_export(&blur, "out", "tiff", true).is_err());
    assert_eq!(p.export_formats(&blur, "out"), ["exr32", "png16"]);

    let cache = EvalCache::default();
    let written = build_marked(
        &p,
        &reg,
        65,
        std::path::Path::new("out"),
        &EvalOptions {
            cache: Some(&cache),
            ..opts
        },
    )
    .unwrap();
    // fbm and levels computed once, although both outputs need them.
    assert_eq!(cache.stats().misses, 4);
    let names: Vec<String> = written
        .iter()
        .map(|w| w.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(written.iter().all(|w| w.starts_with(folder.join("out"))));
    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("out/build.json")).unwrap()).unwrap();
    let files = info["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert!(
        files
            .iter()
            .any(|f| f["data"] == "mask" && f["node"] == slope.as_str())
    );

    // Deleting a node drops its marks; marks survive save/load.
    p.remove_node(&slope).unwrap();
    assert_eq!(p.exports.len(), 2);
    let (loaded, _) = Project::from_json(&p.to_json().unwrap(), &reg).unwrap();
    assert_eq!(loaded.exports, p.exports);
    let _ = fbm;
    std::fs::remove_dir_all(&folder).ok();
}

#[test]
fn relative_output_folder_needs_a_project_folder() {
    let reg = registry();
    let (mut p, [_, _, blur]) = chain(&reg);
    p.set_export(&blur, "out", "exr32", true).unwrap();
    let err = build_marked(&p, &reg, 33, std::path::Path::new("out"), &EvalOptions::default()).unwrap_err();
    assert!(err.to_string().contains("save the project"), "{err}");
}

#[test]
fn file_node_reads_relative_paths_and_notices_changes() {
    let reg = registry();
    let dir = temp_dir("filenode");
    let png = support::test_png(&dir);
    let mut p = Project::default();
    let file = p.graph.add_node(&reg, "primitive.file", [0.0, 0.0]).unwrap();
    p.graph
        .set_param(&reg, &file, "path", ParamValue::Text("heightmap.png".into()))
        .unwrap();
    let opts = EvalOptions {
        base_dir: Some(&dir),
        ..Default::default()
    };
    let g = support::eval_with(&p, &reg, &file, 257, &opts).0;
    // Vertex-aligned: pixel (i, j) of a 257² image lands on sample (i, j).
    let img = image::open(&png).unwrap().into_luma16();
    let expected = img.get_pixel(40, 100).0[0] as f32 / 65535.0 * 2000.0;
    assert!(
        (g.get(40, 100) - expected).abs() < 1e-2,
        "{} vs {expected}",
        g.get(40, 100)
    );

    // Missing file: a clear error, not a crash.
    let spec = GridSpec::full_world(&p.world, 33).unwrap();
    let err = evaluate_node(&p.graph, &reg, &p.world, spec, &file, &EvalOptions::default()).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");

    // Rewriting the file changes the cache key.
    let cache = EvalCache::default();
    let cached = EvalOptions {
        cache: Some(&cache),
        ..opts
    };
    support::eval_with(&p, &reg, &file, 33, &cached);
    support::eval_with(&p, &reg, &file, 33, &cached);
    assert_eq!(cache.stats().misses, 1);
    std::thread::sleep(std::time::Duration::from_millis(20));
    image::ImageBuffer::<image::Luma<u16>, _>::from_pixel(8, 8, image::Luma([1000u16]))
        .save(&png)
        .unwrap();
    let flat = support::eval_with(&p, &reg, &file, 33, &cached).0;
    assert_eq!(cache.stats().misses, 2);
    assert!(
        flat.data
            .iter()
            .all(|&v| (v - 1000.0 / 65535.0 * 2000.0).abs() < 1e-3)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finds_the_terrain_under_a_mask() {
    let reg = registry();
    let (mut p, [_, levels, _]) = chain(&reg);
    let slope = p.graph.add_node(&reg, "data.slope", [0.0, 0.0]).unwrap();
    let select = p.graph.add_node(&reg, "data.select_range", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &levels, "out", &slope, "in").unwrap();
    p.graph.connect(&reg, &slope, "out", &select, "in").unwrap();
    let base = Some((levels.clone(), "out".to_string()));
    assert_eq!(p.graph.base_heightfield(&reg, &slope), base);
    assert_eq!(
        p.graph.base_heightfield(&reg, &select),
        base,
        "found through another mask"
    );
    let lonely = p.graph.add_node(&reg, "noise.perlin", [0.0, 0.0]).unwrap();
    assert_eq!(p.graph.base_heightfield(&reg, &lonely), None);
}

/// The bundled examples (the v0.2 reference landforms) load without warnings,
/// use only built-in nodes, and every marked output builds.
#[test]
fn bundled_examples_load_and_build() {
    let reg = registry();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/examples");
    let mut found = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "otstudio") {
            continue;
        }
        found += 1;
        let (p, warnings) = Project::load(&path, &reg).unwrap();
        assert!(warnings.is_empty(), "{}: {warnings:?}", path.display());
        let viewed = p.ui["viewed_node"].as_str().unwrap();
        let (g, _) = support::eval(&p, &reg, viewed, 129);
        let (lo, hi) = g.min_max();
        assert!(hi > lo, "{}: flat", path.display());
        assert!(
            !p.exports.is_empty(),
            "{}: nothing marked for export",
            path.display()
        );
        let out = temp_dir(&format!("example-{found}"));
        build_marked(&p, &reg, 65, &out, &EvalOptions::default()).unwrap();
        std::fs::remove_dir_all(&out).ok();
    }
    assert_eq!(found, 3);
}
