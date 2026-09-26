//! Vegetation: populations, point sampling, chaining, presets and point export.

mod support;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use support::{registry, smooth_source, temp_dir};
use terrain_core::export::build_marked;
use terrain_core::preset::SpeciesPreset;
use terrain_core::{
    EvalOptions, Grid, GridSpec, NodeRegistry, ParamValue, PointSet, Project, Value, evaluate_node,
};

fn set(p: &mut Project, reg: &NodeRegistry, id: &str, params: &[(&str, ParamValue)]) {
    for (k, v) in params {
        p.graph.set_param(reg, id, k, v.clone()).unwrap();
    }
}

fn f(v: f64) -> ParamValue {
    ParamValue::Float(v)
}

fn text(v: &str) -> ParamValue {
    ParamValue::Text(v.into())
}

fn outputs(p: &Project, reg: &NodeRegistry, id: &str, res: u32) -> BTreeMap<String, Value> {
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    evaluate_node(&p.graph, reg, &p.world, spec, id, &EvalOptions::default()).unwrap()
}

fn points(out: &BTreeMap<String, Value>) -> Arc<PointSet> {
    out["points"].points().unwrap().clone()
}

fn mask(out: &BTreeMap<String, Value>, key: &str) -> Arc<Grid> {
    out[key].grid().clone()
}

/// A population of `type_id` on the smooth test terrain; returns (project, node).
fn population(reg: &NodeRegistry, type_id: &str) -> (Project, String) {
    let mut p = Project::default();
    let src = smooth_source(reg, &mut p);
    let id = p.graph.add_node(reg, type_id, [0.0, 0.0]).unwrap();
    p.graph.connect(reg, &src, "out", &id, "in").unwrap();
    (p, id)
}

/// Smallest distance between any two points (via a grid of buckets).
fn min_distance(points: &PointSet, bucket: f64) -> f64 {
    let mut buckets: BTreeMap<(i64, i64), Vec<(f64, f64)>> = BTreeMap::new();
    for p in points.iter() {
        let (x, y) = (p.x_m as f64, p.y_m as f64);
        buckets
            .entry(((x / bucket).floor() as i64, (y / bucket).floor() as i64))
            .or_default()
            .push((x, y));
    }
    let mut best = f64::INFINITY;
    for (&(bx, by), list) in &buckets {
        for &(x, y) in list {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(other) = buckets.get(&(bx + dx, by + dy)) else {
                        continue;
                    };
                    for &(ox, oy) in other {
                        let d = ((ox - x).powi(2) + (oy - y).powi(2)).sqrt();
                        if d > 0.0 {
                            best = best.min(d);
                        }
                    }
                }
            }
        }
    }
    best
}

#[test]
fn points_keep_their_spacing_and_follow_the_terrain() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.trees");
    set(&mut p, &reg, &id, &[("spacing_m", f(40.0)), ("coverage", f(1.0))]);
    let out = outputs(&p, &reg, &id, 257);
    let pts = points(&out);
    assert!(pts.len() > 1000, "only {} trees", pts.len());
    // Positions are stored as f32: allow their rounding.
    let d = min_distance(&pts, 40.0);
    assert!(d >= 40.0 - 0.01, "two trees {d} m apart");
    // Every point stands on the terrain, inside the world, within its ranges.
    let terrain_spec = GridSpec::full_world(&p.world, 257).unwrap();
    let src = p.graph.link_into(&id, "in").unwrap().from.0.clone();
    let height = outputs(&p, &reg, &src, 257)["out"].grid().clone();
    assert_eq!(height.spec, terrain_spec);
    for pt in pts.iter() {
        assert!((0.0..=8192.0).contains(&pt.x_m) && (0.0..=8192.0).contains(&pt.y_m));
        let h = height.sample_bilinear_m(pt.x_m as f64, pt.y_m as f64);
        assert!(
            (pt.z_m - h).abs() < 0.01,
            "point at {} m on ground at {h} m",
            pt.z_m
        );
        assert!((0.0..360.0).contains(&pt.rotation_deg));
        assert!((0.8..=1.2).contains(&pt.scale));
    }
    assert_eq!(pts.species, vec!["tree".to_string()]);
}

#[test]
fn points_are_the_same_for_any_thread_count() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.shrubs");
    set(&mut p, &reg, &id, &[("spacing_m", f(20.0))]);
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
    let a = one.install(|| points(&outputs(&p, &reg, &id, 129)));
    let b = many.install(|| points(&outputs(&p, &reg, &id, 129)));
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn points_stay_put_when_the_resolution_changes() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.trees");
    set(&mut p, &reg, &id, &[("spacing_m", f(30.0))]);
    let lo = points(&outputs(&p, &reg, &id, 513));
    let hi = points(&outputs(&p, &reg, &id, 2049));
    let key = |s: &PointSet| -> HashSet<(u32, u32)> {
        s.iter().map(|p| (p.x_m.to_bits(), p.y_m.to_bits())).collect()
    };
    let (a, b) = (key(&lo), key(&hi));
    let shared = a.intersection(&b).count() as f64;
    let share = shared / a.len().max(b.len()) as f64;
    println!(
        "{} and {} points, {:.1}% at identical positions",
        a.len(),
        b.len(),
        share * 100.0
    );
    assert!(
        share > 0.9,
        "only {:.1}% of points kept their place",
        share * 100.0
    );
}

/// The exit-criterion graph in miniature: pine high and dry, birch on damp
/// low ground avoiding the pines, shrubs intermingling with both.
fn chained(reg: &NodeRegistry) -> (Project, [String; 3]) {
    let mut p = Project::default();
    let terrain = smooth_source(reg, &mut p);
    let wet = p.graph.add_node(reg, "data.wetness", [0.0, 0.0]).unwrap();
    p.graph.connect(reg, &terrain, "out", &wet, "in").unwrap();
    let pine = p.graph.add_node(reg, "vegetation.trees", [0.0, 0.0]).unwrap();
    let birch = p.graph.add_node(reg, "vegetation.trees", [0.0, 0.0]).unwrap();
    let shrubs = p.graph.add_node(reg, "vegetation.shrubs", [0.0, 0.0]).unwrap();
    for id in [&pine, &birch, &shrubs] {
        p.graph.connect(reg, &terrain, "out", id, "in").unwrap();
        p.graph.connect(reg, &wet, "wetness", id, "water").unwrap();
    }
    p.graph
        .connect(reg, &pine, "occupied", &birch, "occupied")
        .unwrap();
    p.graph
        .connect(reg, &birch, "occupied", &shrubs, "occupied")
        .unwrap();
    set(
        &mut p,
        reg,
        &pine,
        &[
            ("species", text("scots_pine")),
            ("height_min_m", f(1000.0)),
            ("height_max_m", f(1900.0)),
            ("water_preference", f(-0.3)),
            ("spacing_m", f(30.0)),
        ],
    );
    set(
        &mut p,
        reg,
        &birch,
        &[
            ("species", text("silver_birch")),
            ("height_max_m", f(1100.0)),
            ("water_preference", f(0.8)),
            ("spacing_m", f(25.0)),
        ],
    );
    set(
        &mut p,
        reg,
        &shrubs,
        &[
            ("species", text("hazel")),
            ("occupied_mode", text("intermingle")),
            ("spacing_m", f(15.0)),
        ],
    );
    (p, [pine, birch, shrubs])
}

#[test]
fn chained_populations_take_different_habitats() {
    let reg = registry();
    let (p, [pine, birch, shrubs]) = chained(&reg);
    let (po, bo, so) = (
        outputs(&p, &reg, &pine, 257),
        outputs(&p, &reg, &birch, 257),
        outputs(&p, &reg, &shrubs, 257),
    );
    let mean_z = |s: &PointSet| s.iter().map(|p| p.z_m as f64).sum::<f64>() / s.len().max(1) as f64;
    let (pp, bp, sp) = (points(&po), points(&bo), points(&so));
    println!(
        "pine {} points at {:.0} m, birch {} at {:.0} m, shrubs {} at {:.0} m",
        pp.len(),
        mean_z(&pp),
        bp.len(),
        mean_z(&bp),
        sp.len(),
        mean_z(&sp)
    );
    assert!(pp.len() > 200 && bp.len() > 200 && sp.len() > 200);
    assert!(
        mean_z(&pp) > mean_z(&bp) + 300.0,
        "pines should grow well above birches"
    );
    // Birches avoid the pines: little of their cover overlaps.
    let (dp, db) = (mask(&po, "density"), mask(&bo, "density"));
    let overlap: f64 = dp.data.iter().zip(&db.data).map(|(a, b)| a.min(*b) as f64).sum();
    let birch_total: f64 = db.data.iter().map(|v| *v as f64).sum();
    assert!(
        overlap / birch_total < 0.12,
        "birch overlaps pine by {:.2}",
        overlap / birch_total
    );
    // Birches seek water: their water influence is higher where they grow.
    let water = mask(&bo, "water");
    let weighted: f64 = db.data.iter().zip(&water.data).map(|(d, w)| (d * w) as f64).sum();
    assert!(weighted / birch_total > water.mean() + 0.05);
    // Occupied accumulates down the chain.
    let (occ_b, occ_s) = (mask(&bo, "occupied"), mask(&so, "occupied"));
    assert!(occ_s.mean() > occ_b.mean() && occ_b.mean() > dp.mean());
    // Each population has its species name.
    assert_eq!(pp.species, vec!["scots_pine".to_string()]);
    assert_eq!(sp.species, vec!["hazel".to_string()]);
}

#[test]
fn dead_zones_clear_steep_ground_and_debris_collects_below_it() {
    let reg = registry();
    let mut p = Project::default();
    let cone = p.graph.add_node(&reg, "primitive.cone", [0.0, 0.0]).unwrap();
    set(
        &mut p,
        &reg,
        &cone,
        &[("height_m", f(1800.0)), ("radius_m", f(2500.0))],
    );
    let trees = p.graph.add_node(&reg, "vegetation.trees", [0.0, 0.0]).unwrap();
    let rocks = p.graph.add_node(&reg, "vegetation.debris", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &cone, "out", &trees, "in").unwrap();
    p.graph.connect(&reg, &cone, "out", &rocks, "in").unwrap();
    p.graph
        .connect(&reg, &trees, "dead_zones", &rocks, "dead_zones")
        .unwrap();
    set(
        &mut p,
        &reg,
        &trees,
        &[
            ("dead_slope_deg", f(30.0)),
            ("dead_zones", f(1.0)),
            ("height_max_m", f(5000.0)),
        ],
    );
    let t = outputs(&p, &reg, &trees, 257);
    let r = outputs(&p, &reg, &rocks, 257);
    let (dead, density, rock) = (mask(&t, "dead_zones"), mask(&t, "density"), mask(&r, "density"));
    // A 1,800 m cone of radius 2,500 m has 36° sides: dead, with no trees.
    let (mid, far) = (1250.0, 3800.0);
    let c = 4096.0;
    assert!(dead.sample_bilinear_m(c + mid, c) > 0.5);
    assert!(density.sample_bilinear_m(c + mid, c) < 0.1);
    assert!(density.sample_bilinear_m(c + far, c) > 0.3);
    assert!(rock.sample_bilinear_m(c + mid, c) > 0.3);
    assert!(!points(&r).is_empty());
    assert_eq!(points(&r).species, vec!["rock".to_string()]);
}

#[test]
fn pack_masks_keeps_values_unchanged() {
    let reg = registry();
    let (mut p, pine) = population(&reg, "vegetation.trees");
    let pack = p.graph.add_node(&reg, "output.pack_masks", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &pine, "density", &pack, "r").unwrap();
    p.graph.connect(&reg, &pine, "dead_zones", &pack, "b").unwrap();
    let packed = outputs(&p, &reg, &pack, 65)["out"].color().unwrap().clone();
    let t = outputs(&p, &reg, &pine, 65);
    assert_eq!(packed.channel(0).data, mask(&t, "density").data);
    assert_eq!(packed.channel(2).data, mask(&t, "dead_zones").data);
    assert!(packed.channel(1).data.iter().all(|v| *v == 0.0));
    assert!(packed.channel(3).data.iter().all(|v| *v == 0.0));
}

#[test]
fn points_export_as_csv_and_json() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.trees");
    set(
        &mut p,
        &reg,
        &id,
        &[("spacing_m", f(60.0)), ("species", text("scots_pine"))],
    );
    p.set_export(&id, "points", "csv", true).unwrap();
    p.set_export(&id, "points", "json", true).unwrap();
    p.set_export(&id, "density", "png8", true).unwrap();
    let dir = temp_dir("points-export");
    let written = build_marked(&p, &reg, 129, &dir, &EvalOptions::default()).unwrap();
    assert_eq!(written.len(), 4);
    let pts = points(&outputs(&p, &reg, &id, 129));
    let csv_path = written.iter().find(|p| p.extension().unwrap() == "csv").unwrap();
    let csv = std::fs::read_to_string(csv_path).unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("x,y,z,rotation_deg,scale,species"));
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), pts.len());
    for (row, pt) in rows.iter().zip(pts.iter()) {
        let x: f32 = row[0].parse().unwrap();
        let z: f32 = row[2].parse().unwrap();
        assert!((x - pt.x_m).abs() < 0.001 && (z - pt.z_m).abs() < 0.001);
        assert_eq!(row[5], "scots_pine");
    }
    let json_path = written
        .iter()
        .find(|p| p.extension().unwrap() == "json" && !p.ends_with("build.json"));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(json_path.unwrap()).unwrap()).unwrap();
    assert_eq!(json["count"].as_u64().unwrap() as usize, pts.len());
    assert_eq!(json["points"].as_array().unwrap().len(), pts.len());
    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("build.json")).unwrap()).unwrap();
    let files = info["files"].as_array().unwrap();
    let csv_entry = files.iter().find(|f| f["format"] == "csv").unwrap();
    assert_eq!(csv_entry["data"], "point_set");
    assert_eq!(csv_entry["points"].as_u64().unwrap() as usize, pts.len());
    assert_eq!(csv_entry["species"][0], "scots_pine");
    assert!(
        csv_entry["encoding"]
            .as_str()
            .unwrap()
            .starts_with("x,y metres from the world origin")
    );
    let png_entry = files.iter().find(|f| f["format"] == "png8").unwrap();
    assert!(png_entry.get("points").is_none());
}

#[test]
fn mismatched_export_formats_are_refused_before_building() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.trees");
    p.set_export(&id, "points", "exr32", true).unwrap();
    let dir = temp_dir("points-mismatch");
    let err = build_marked(&p, &reg, 65, &dir, &EvalOptions::default()).unwrap_err();
    assert!(err.to_string().contains("can't be exported as exr32"), "{err}");
    let mut p2 = p.clone();
    p2.set_export(&id, "points", "exr32", false).unwrap();
    p2.set_export(&id, "density", "csv", true).unwrap();
    assert!(build_marked(&p2, &reg, 65, &dir, &EvalOptions::default()).is_err());
}

#[test]
fn spacing_too_small_for_the_world_is_an_error() {
    let reg = registry();
    let (mut p, id) = population(&reg, "vegetation.grass");
    set(&mut p, &reg, &id, &[("spacing_m", f(0.25))]);
    let spec = GridSpec::full_world(&p.world, 65).unwrap();
    let err = evaluate_node(&p.graph, &reg, &p.world, spec, &id, &EvalOptions::default()).unwrap_err();
    assert!(err.to_string().contains("Raise the spacing"), "{err}");
}

/// Exit criterion: over a million points in under ten seconds. Grass on the
/// default 8 km world, fully covered, at a 1,025² build.
#[test]
fn a_million_points_in_under_ten_seconds() {
    let reg = registry();
    let mut p = Project::default();
    let flat = p.graph.add_node(&reg, "primitive.constant", [0.0, 0.0]).unwrap();
    let grass = p.graph.add_node(&reg, "vegetation.grass", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &flat, "out", &grass, "in").unwrap();
    set(
        &mut p,
        &reg,
        &grass,
        &[
            ("spacing_m", f(4.0)),
            ("coverage", f(1.0)),
            ("health", f(1.0)),
            ("patches", f(0.0)),
            ("randomness", f(0.0)),
            ("dead_zones", f(0.0)),
            ("water_preference", f(0.0)),
        ],
    );
    let started = Instant::now();
    let out = outputs(&p, &reg, &grass, 1025);
    let secs = started.elapsed().as_secs_f64();
    let n = points(&out).len();
    println!("{n} points in {secs:.2} s");
    assert!(n >= 1_000_000, "only {n} points");
    assert!(secs < 10.0, "{n} points took {secs:.1} s");
}

#[test]
fn bundled_species_presets_apply_cleanly() {
    let reg = registry();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/examples/species");
    let mut biomes = HashSet::new();
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "yaml") {
            continue;
        }
        let preset = SpeciesPreset::parse(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let schema = reg
            .schema(&preset.node)
            .unwrap_or_else(|| panic!("{}: unknown node {}", path.display(), preset.node));
        let (values, warnings) = preset.values_for(schema);
        assert!(warnings.is_empty(), "{}: {warnings:?}", path.display());
        assert_eq!(values.len(), preset.params.len(), "{}", path.display());
        assert!(!preset.name.is_empty() && !preset.description.is_empty());
        assert!(preset.params.contains_key("species"), "{}", path.display());
        // Values are within range: validation didn't have to clamp them.
        for (key, v) in &values {
            assert_eq!(
                v.as_f64().or(Some(0.0)),
                preset.params[key].as_f64().or(Some(0.0)),
                "{}: {key} was clamped",
                path.display()
            );
        }
        biomes.insert(preset.biome.clone());
        count += 1;
    }
    assert!(count >= 15, "{count} presets");
    for b in ["temperate", "alpine", "desert", "tropical"] {
        assert!(biomes.contains(b), "no {b} presets");
    }
}
