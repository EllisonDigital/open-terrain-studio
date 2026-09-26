//! Tiled evaluation (v0.8): tiles put back together match an untiled
//! evaluation, exactly for local nodes and closely for global ones.

mod support;

use std::collections::BTreeMap;

use terrain_core::tiled::{Tile, evaluate_tiled, tiles};
use terrain_core::{
    EvalContext, EvalOptions, GridSpec, NodeRegistry, ParamValue, Project, Reach, Value, evaluate_node,
};

use support::{registry, temp_dir};

/// Every output of `node`, evaluated in tiles of `tile` samples and put back
/// together (points: all tiles' points).
fn tiled_outputs(
    p: &Project,
    reg: &NodeRegistry,
    node: &str,
    res: u32,
    tile: u32,
) -> BTreeMap<String, Value> {
    let full = GridSpec::full_world(&p.world, res).unwrap();
    let schema = reg.schema(&p.graph.node(node).unwrap().type_id).unwrap();
    let targets: Vec<(String, String)> = schema
        .outputs
        .iter()
        .map(|o| (node.to_string(), o.key.clone()))
        .collect();
    let mut parts: BTreeMap<String, Vec<(Tile, Value)>> = BTreeMap::new();
    evaluate_tiled(
        &p.graph,
        reg,
        &p.world,
        full,
        &targets,
        tile,
        &EvalOptions::default(),
        &mut |t, _, port, v| {
            parts.entry(port.to_string()).or_default().push((*t, v));
            Ok(())
        },
    )
    .unwrap_or_else(|e| panic!("{node}: {e}"));
    parts
        .into_iter()
        .map(|(port, tiles)| (port, assemble(full, tiles)))
        .collect()
}

/// Put tile cores back into one value over `full`.
fn assemble(full: GridSpec, tiles: Vec<(Tile, Value)>) -> Value {
    use std::sync::Arc;
    use terrain_core::{ColorGrid, Grid, PointSet};
    let first = &tiles[0].1;
    match first {
        Value::Points(p) => {
            let mut all = PointSet::new(full, "");
            all.species = p.species.clone();
            for (_, v) in &tiles {
                for q in v.points().unwrap().iter() {
                    all.push(q);
                }
            }
            Value::Points(Arc::new(all))
        }
        Value::ColorMap(_) => {
            let mut data = vec![f32::NAN; full.len() * 4];
            for (t, v) in &tiles {
                let c = v.color().unwrap();
                paste(&mut data, 4, full, t.core, &c.data);
            }
            Value::ColorMap(Arc::new(ColorGrid { spec: full, data }))
        }
        v => {
            let ty = v.port_type();
            let mut data = vec![f32::NAN; full.len()];
            for (t, v) in &tiles {
                paste(&mut data, 1, full, t.core, &v.grid().data);
            }
            let g = Arc::new(Grid { spec: full, data });
            match ty {
                terrain_core::PortType::Mask => Value::Mask(g),
                _ => Value::Heightfield(g),
            }
        }
    }
}

fn paste(dst: &mut [f32], ch: usize, full: GridSpec, core: GridSpec, src: &[f32]) {
    assert_eq!(src.len(), core.len() * ch);
    let o = core.offset();
    let w = core.width as usize * ch;
    for j in 0..core.height as usize {
        let at = ((o[1] as usize + j) * full.width as usize + o[0] as usize) * ch;
        assert!(dst[at..at + w].iter().all(|v| v.is_nan()), "tiles overlap");
        dst[at..at + w].copy_from_slice(&src[j * w..(j + 1) * w]);
    }
}

/// Points sorted by position, for comparing sets made in different orders.
fn sorted_points(v: &Value) -> Vec<[u32; 5]> {
    let mut pts: Vec<[u32; 5]> = v
        .points()
        .unwrap()
        .iter()
        .map(|q| [q.x_m, q.y_m, q.z_m, q.rotation_deg, q.scale].map(f32::to_bits))
        .collect();
    pts.sort();
    pts
}

fn reach_of(p: &Project, reg: &NodeRegistry, node: &str, res: u32) -> Reach {
    let n = p.graph.node(node).unwrap();
    let kind = reg.get(&n.type_id).unwrap();
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let ctx = EvalContext::new(&p.world, spec, 0, node, kind.schema(), &n.params, BTreeMap::new());
    kind.reach(&ctx)
}

#[test]
fn tiles_cover_every_sample_once() {
    let full = GridSpec::new(1000, 7, [0.0, 0.0], [999.0, 6.0]);
    let t = tiles(full, 333);
    // 1000 = 333 + 333 + 333 + 1: the 1-sample remainder joins the last tile.
    let widths: Vec<u32> = t.iter().map(|t| t.core.width).collect();
    assert_eq!(widths, vec![333, 333, 334]);
    assert!(t.iter().all(|t| t.core.height == 7));
    // Window positions are bit-identical to the whole grid's.
    let last = t[2].core;
    assert_eq!(last.x_m(0).to_bits(), full.x_m(666).to_bits());
    assert_eq!(last.x_m(333).to_bits(), full.x_m(999).to_bits());
    assert!(t[0].owns(332.5, 0.0) && !t[0].owns(333.0, 0.0) && t[1].owns(333.0, 0.0));
    assert!(t[2].owns(999.0, 6.0));
}

/// The main check on every node's [`NodeKind::reach`]: for each node type
/// that says it is local, a tiled evaluation (tiles that don't divide the
/// grid evenly) is bit-identical to an untiled one. Global nodes must come
/// close.
#[test]
fn every_node_type_matches_untiled() {
    let reg = registry();
    let dir = temp_dir("tiled-all");
    let res = 257;
    let mut local = 0;
    let mut failures = Vec::new();
    for schema in reg.schemas() {
        let (p, id) = support::project_for(&reg, &schema.type_id, &dir);
        let spec = GridSpec::full_world(&p.world, res).unwrap();
        let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &id, &EvalOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", schema.type_id));
        let tiled = tiled_outputs(&p, &reg, &id, res, 60);
        let is_local = matches!(reach_of(&p, &reg, &id, res), Reach::Local(_));
        local += is_local as usize;
        for port in &schema.outputs {
            let (a, b) = (&whole[&port.key], &tiled[&port.key]);
            let what = format!("{}.{}", schema.type_id, port.key);
            if let Value::Points(_) = a {
                let (pa, pb) = (sorted_points(a), sorted_points(b));
                let same = pa.iter().filter(|q| pb.binary_search(q).is_ok()).count();
                let ok = if is_local {
                    pa == pb
                } else {
                    pa.len().abs_diff(pb.len()) as f64 <= 0.02 * pa.len() as f64 + 2.0
                };
                if !ok {
                    failures.push(format!(
                        "{what}: {} points untiled, {} tiled, {same} in both",
                        pa.len(),
                        pb.len()
                    ));
                }
                continue;
            }
            let (sa, sb) = (a.samples(), b.samples());
            assert_eq!(sa.len(), sb.len(), "{what}");
            assert!(sb.iter().all(|v| !v.is_nan()), "{what}: a sample no tile filled");
            if is_local {
                let differing = sa
                    .iter()
                    .zip(sb)
                    .filter(|(x, y)| x.to_bits() != y.to_bits())
                    .count();
                if differing > 0 {
                    let worst = sa
                        .iter()
                        .zip(sb)
                        .map(|(x, y)| (x - y).abs())
                        .fold(0.0f32, f32::max);
                    failures.push(format!("{what}: {differing} samples differ, by up to {worst}"));
                }
            } else {
                // Close: mean difference under 1% of the value range.
                let (lo, hi) = sa
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
                let mean =
                    sa.iter().zip(sb).map(|(x, y)| (x - y).abs() as f64).sum::<f64>() / sa.len() as f64;
                if mean > 0.01 * (hi - lo).max(1.0e-3) as f64 {
                    failures.push(format!(
                        "{what}: mean difference {mean} over a range of {}",
                        hi - lo
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "tiled results differ:
{}",
        failures.join(
            "
"
        )
    );
    assert!(local >= 40, "only {local} node types are local");
}

/// Auto-range Levels is global, but its world pass only needs the exact
/// input range, so tiles match an untiled evaluation exactly.
#[test]
fn auto_levels_uses_the_whole_build_range() {
    let reg = registry();
    let mut p = Project::default();
    let fbm = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let levels = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &fbm, "out", &levels, "in").unwrap();
    assert_eq!(reach_of(&p, &reg, &levels, 513), Reach::Global);
    let spec = GridSpec::full_world(&p.world, 513).unwrap();
    let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &levels, &EvalOptions::default()).unwrap();
    let tiled = tiled_outputs(&p, &reg, &levels, 513, 100);
    assert_eq!(whole["out"].samples(), tiled["out"].samples());

    // Manual range: point-wise.
    p.graph
        .set_param(&reg, &levels, "auto_input", ParamValue::Bool(false))
        .unwrap();
    assert_eq!(reach_of(&p, &reg, &levels, 513), Reach::Local(0.0));
}

/// Hydraulic erosion below its simulation resolution: the world pass runs
/// on the same simulation grid as an untiled build, fed the same filtered
/// terrain, so heights agree to well under a metre.
#[test]
fn eroded_tiles_match_untiled_erosion() {
    let reg = registry();
    let dir = temp_dir("tiled-erosion");
    let (p, id) = support::project_for(&reg, "simulate.hydraulic", &dir);
    let res = 513; // 16 m cells; simulated at 32 m
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &id, &EvalOptions::default()).unwrap();
    let tiled = tiled_outputs(&p, &reg, &id, res, 128);
    let (a, b) = (whole["height"].samples(), tiled["height"].samples());
    let worst = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(worst < 0.05, "worst height difference {worst} m");
}

/// Every global node when the build is finer than its world-pass grid (the
/// real tiled case): tiled results stay close to untiled ones.
#[test]
fn global_nodes_close_below_simulation_resolution() {
    let reg = registry();
    let dir = temp_dir("tiled-global");
    let res = 513; // 16 m cells; drainage and erosion at 32 m
    let mut report = Vec::new();
    for schema in reg.schemas() {
        let (p, id) = support::project_for(&reg, &schema.type_id, &dir);
        if reach_of(&p, &reg, &id, res) != Reach::Global {
            continue;
        }
        let spec = GridSpec::full_world(&p.world, res).unwrap();
        let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &id, &EvalOptions::default()).unwrap();
        let tiled = tiled_outputs(&p, &reg, &id, res, 128);
        for port in &schema.outputs {
            let (sa, sb) = (whole[&port.key].samples(), tiled[&port.key].samples());
            let (lo, hi) = sa
                .iter()
                .fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
            let span = (hi - lo).max(1.0e-3) as f64;
            let mean = sa.iter().zip(sb).map(|(x, y)| (x - y).abs() as f64).sum::<f64>() / sa.len() as f64;
            report.push((format!("{}.{}", schema.type_id, port.key), mean / span));
        }
    }
    let text: Vec<String> = report
        .iter()
        .map(|(k, v)| format!("{k}: {:.4}%", v * 100.0))
        .collect();
    println!("{}", text.join("\n"));
    assert!(report.len() >= 20, "{report:?}");
    let bad: Vec<&String> = report.iter().filter(|(_, v)| *v > 0.01).map(|(k, _)| k).collect();
    assert!(
        bad.is_empty(),
        "over 1% mean difference: {bad:?}\n{}",
        text.join("\n")
    );
}

/// A build above the tiling threshold (4,609² in tiles of 1,500) is
/// bit-identical to an untiled evaluation, auto-range Levels included.
#[test]
fn large_builds_are_tiled_and_match() {
    use terrain_core::export::build_marked;
    use terrain_core::import::read_height_image;
    let reg = registry();
    let mut p = Project::default();
    let fbm = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let blur = p.graph.add_node(&reg, "adjust.blur", [0.0, 0.0]).unwrap();
    let levels = p.graph.add_node(&reg, "adjust.levels", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &fbm, "out", &blur, "in").unwrap();
    p.graph.connect(&reg, &blur, "out", &levels, "in").unwrap();
    p.graph
        .set_param(&reg, &fbm, "octaves", ParamValue::Int(3))
        .unwrap();
    p.set_export(&levels, "out", "exr32", true).unwrap();
    p.build.tile_size = 1500;
    let res = 4609;
    let dir = temp_dir("tiled-build");
    let written = build_marked(&p, &reg, res, &dir, &EvalOptions::default()).unwrap();
    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("build.json")).unwrap()).unwrap();
    assert_eq!(info["computed_in_tiles"], 1500);
    let exr = written
        .iter()
        .find(|w| w.extension().is_some_and(|e| e == "exr"))
        .unwrap();
    let image = read_height_image(exr).unwrap();
    assert_eq!((image.width, image.height), (res, res));

    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &levels, &EvalOptions::default()).unwrap();
    let expected = whole["out"].samples();
    let differing = image
        .data
        .iter()
        .zip(expected)
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();
    assert_eq!(differing, 0, "{differing} samples differ from the untiled result");
    assert!(
        std::fs::read_dir(&dir)
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".raw"))
    );
}

/// Tile files share their edge samples, are named with the pattern, and are
/// listed in build.json with their place in the whole build.
#[test]
fn builds_write_tile_files() {
    use terrain_core::export::build_marked;
    use terrain_core::import::read_height_image;
    use terrain_core::project::FileTiles;
    let reg = registry();
    let mut p = Project::default();
    let fbm = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    p.set_export(&fbm, "out", "exr32", true).unwrap();
    p.build.file_tiles = Some(FileTiles {
        size: 65,
        pattern: "{name}_x{x}_y{y}".into(),
    });
    let dir = temp_dir("tile-files");
    let res = 129; // 2 × (65 - 1) + 1
    build_marked(&p, &reg, res, &dir, &EvalOptions::default()).unwrap();
    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("build.json")).unwrap()).unwrap();
    assert_eq!(info["file_tiles"]["columns"], 2);
    assert_eq!(info["file_tiles"]["even"], true);
    let files = info["files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    let spec = GridSpec::full_world(&p.world, res).unwrap();
    let whole = evaluate_node(&p.graph, &reg, &p.world, spec, &fbm, &EvalOptions::default()).unwrap();
    let whole = whole["out"].grid();
    for f in files {
        let name = f["file"].as_str().unwrap();
        let (tx, ty) = (f["tile"][0].as_u64().unwrap(), f["tile"][1].as_u64().unwrap());
        assert!(name.ends_with(&format!("_x{tx}_y{ty}.exr")), "{name}");
        let origin = [
            f["tile_origin"][0].as_u64().unwrap() as u32,
            f["tile_origin"][1].as_u64().unwrap() as u32,
        ];
        assert_eq!(origin, [tx as u32 * 64, ty as u32 * 64]);
        let img = read_height_image(&dir.join(name)).unwrap();
        assert_eq!((img.width, img.height), (65, 65));
        for j in 0..65 {
            for i in 0..65 {
                assert_eq!(
                    img.data[(j * 65 + i) as usize],
                    whole.get(origin[0] + i, origin[1] + j)
                );
            }
        }
    }
}
