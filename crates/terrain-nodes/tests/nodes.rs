//! Checks every registered node type: finite output, masks in 0..1,
//! determinism across thread counts, resolution independence and golden
//! hashes (so an accidental change to any node's output fails CI).

mod support;

use std::collections::BTreeMap;

use support::{bits, eval, project_for, registry, temp_dir};
use terrain_core::PortType;
use terrain_core::seed::fnv1a64;

#[test]
fn every_node_output_is_finite_and_masks_are_normalised() {
    let reg = registry();
    let dir = temp_dir("finite");
    for schema in reg.schemas() {
        let (p, id) = project_for(&reg, &schema.type_id, &dir);
        let (g, ty) = eval(&p, &reg, &id, 65);
        assert!(
            g.data.iter().all(|v| v.is_finite()),
            "{} produced NaN/inf",
            schema.type_id
        );
        if ty == PortType::Mask {
            assert!(
                g.data.iter().all(|v| (0.0..=1.0).contains(v)),
                "{} mask outside 0..1",
                schema.type_id
            );
        }
    }
}

#[test]
fn every_node_is_deterministic_across_thread_counts() {
    let reg = registry();
    let dir = temp_dir("threads");
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
    for schema in reg.schemas() {
        let (p, id) = project_for(&reg, &schema.type_id, &dir);
        let a = one.install(|| eval(&p, &reg, &id, 97).0);
        let b = many.install(|| eval(&p, &reg, &id, 97).0);
        assert_eq!(
            bits(&a),
            bits(&b),
            "{} differs between 1 and 8 threads",
            schema.type_id
        );
    }
}

/// Allowed mismatch between 513² and 2049², as a fraction of the output's
/// value range: (mean, 99th percentile). Pure functions of position match
/// exactly; neighbourhood operations differ by their discretisation.
fn tolerance(type_id: &str) -> (f64, f64) {
    match type_id {
        // Derivatives and thresholds of derivatives.
        "data.slope" | "data.aspect" => (0.01, 0.08),
        "data.curvature" | "adjust.sharpen" => (0.01, 0.08),
        // Distances are exact to about one low-res cell (16 m of a 500 m fade).
        "data.distance" => (0.01, 0.05),
        _ => (0.002, 0.01),
    }
}

#[test]
fn every_node_is_resolution_independent() {
    // 513 and 2049 samples: every 4th high-res sample sits exactly on a low-res one.
    let reg = registry();
    let dir = temp_dir("resolution");
    let mut report = Vec::new();
    for schema in reg.schemas() {
        let (p, id) = project_for(&reg, &schema.type_id, &dir);
        let (lo, _) = eval(&p, &reg, &id, 513);
        let (hi, _) = eval(&p, &reg, &id, 2049);
        let (min, max) = hi.min_max();
        let range = ((max - min) as f64).max(1e-3);
        let mut diffs: Vec<f64> = (0..513u32)
            .flat_map(|j| (0..513u32).map(move |i| (i, j)))
            .map(|(i, j)| (lo.get(i, j) - hi.get(i * 4, j * 4)).abs() as f64 / range)
            .collect();
        diffs.sort_by(f64::total_cmp);
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        let p99 = diffs[diffs.len() * 99 / 100];
        let (tol_mean, tol_p99) = tolerance(&schema.type_id);
        report.push(format!("{:<22} mean {mean:.5}  p99 {p99:.5}", schema.type_id));
        assert!(
            mean <= tol_mean && p99 <= tol_p99,
            "{} is not resolution independent: mean {mean:.5} (max {tol_mean}), p99 {p99:.5} (max {tol_p99})",
            schema.type_id
        );
    }
    println!("{}", report.join("\n"));
}

/// Golden hashes of every node's default output at 65². Regenerate after a
/// deliberate change with `OTS_BLESS=1 cargo test -p terrain-nodes --test nodes golden`
/// and explain the change in the merge request: it changes users' terrains.
#[test]
fn golden_hashes() {
    let reg = registry();
    let dir = temp_dir("golden");
    let mut hashes = BTreeMap::new();
    for schema in reg.schemas() {
        let (p, id) = project_for(&reg, &schema.type_id, &dir);
        let (g, _) = eval(&p, &reg, &id, 65);
        let bytes: Vec<u8> = g.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        hashes.insert(schema.type_id.clone(), format!("{:016x}", fnv1a64(&bytes)));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/node_hashes.json");
    let json = serde_json::to_string_pretty(&hashes).unwrap() + "\n";
    if std::env::var_os("OTS_BLESS").is_some() {
        std::fs::write(&path, json).unwrap();
        return;
    }
    let stored: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("tests/golden/node_hashes.json missing: run with OTS_BLESS=1"),
    )
    .unwrap();
    let mut changed = Vec::new();
    for (k, v) in &hashes {
        match stored.get(k) {
            Some(s) if s == v => {}
            Some(_) => changed.push(format!("{k} changed")),
            None => changed.push(format!("{k} has no stored hash")),
        }
    }
    assert!(
        changed.is_empty(),
        "node outputs changed:\n  {}\nIf deliberate, re-run with OTS_BLESS=1.",
        changed.join("\n  ")
    );
}
