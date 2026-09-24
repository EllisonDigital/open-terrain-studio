//! End-to-end tests: graph editing, evaluation, determinism, resolution
//! independence, project files and export.

use terrain_core::export::{ExportFormat, ExportRequest, export_node};
use terrain_core::{
    CoreError, EvalOptions, GridSpec, NodeRegistry, ParamValue, Project, Value, World, evaluate_node,
};

fn registry() -> NodeRegistry {
    terrain_nodes::registry()
}

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
    let world = World::default();
    for schema in reg.schemas() {
        let mut p = Project::default();
        let id = p.graph.add_node(&reg, &schema.type_id, [0.0, 0.0]).unwrap();
        // Feed every required input from a noise node.
        for input in schema.inputs.iter().filter(|i| !i.optional) {
            let src = p.graph.add_node(&reg, "noise.perlin", [0.0, 0.0]).unwrap();
            p.graph.connect(&reg, &src, "out", &id, &input.key).unwrap();
        }
        let spec = GridSpec::full_world(&world, 64).unwrap();
        let out = evaluate_node(&p.graph, &reg, &world, spec, &id, &EvalOptions::default())
            .unwrap_or_else(|e| panic!("{} failed: {e}", schema.type_id));
        for port in &schema.outputs {
            let v = out
                .get(&port.key)
                .unwrap_or_else(|| panic!("{} missing {}", schema.type_id, port.key));
            assert!(
                v.grid().data.iter().all(|x| x.is_finite()),
                "{} produced NaN",
                schema.type_id
            );
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
            None,
            None,
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
