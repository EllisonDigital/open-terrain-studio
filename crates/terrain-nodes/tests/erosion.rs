use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use terrain_core::error::CoreError;
use terrain_core::{EvalContext, Grid, GridSpec, NodeKind, Outputs, ParamValue, Value, World};
use terrain_nodes::erosion::{Hydraulic, Thermal};

fn world() -> World {
    World {
        size_m: [1024.0, 1024.0],
        ..World::default()
    }
}
fn terrain(res: u32) -> Grid {
    Grid::from_fn(GridSpec::full_world(&world(), res).unwrap(), |x, y| {
        let ridge = 250.0 * libm::exp(-((x - 512.0) / 160.0).powi(2));
        (100.0 + ridge + y * 0.15 + 25.0 * libm::sin(x / 90.0) * libm::cos(y / 130.0)) as f32
    })
}
fn run(
    node: &dyn NodeKind,
    grid: &Grid,
    params: &[(&str, f64)],
    mask: Option<Grid>,
    hardness: Option<Grid>,
) -> Outputs {
    let world = world();
    let params = params
        .iter()
        .map(|(k, v)| ((*k).into(), ParamValue::Float(*v)))
        .collect();
    let mut inputs = BTreeMap::from([("in".into(), Value::Heightfield(Arc::new(grid.clone())))]);
    if let Some(g) = mask {
        inputs.insert("mask".into(), Value::Mask(Arc::new(g)));
    }
    if let Some(g) = hardness {
        inputs.insert("hardness".into(), Value::Mask(Arc::new(g)));
    }
    node.evaluate(&EvalContext::new(
        &world,
        grid.spec,
        7,
        "erosion",
        node.schema(),
        &params,
        inputs,
    ))
    .unwrap()
}

#[test]
fn deterministic_across_thread_counts_all_outputs() {
    let g = terrain(65);
    for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
        let a = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| run(node, &g, &[], None, None));
        let b = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap()
            .install(|| run(node, &g, &[], None, None));
        for (key, value) in a {
            assert_eq!(
                value.grid().data.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b[&key]
                    .grid()
                    .data
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                "{key}"
            );
        }
    }
}
#[test]
fn zero_duration_flat_ground_and_zero_strength_are_identity() {
    let shaped = terrain(33);
    let flat = Grid::filled(shaped.spec, 123.0);
    for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
        for (g, params, mask) in [
            (&shaped, vec![("duration_s", 0.0), ("duration_kyr", 0.0)], None),
            (&shaped, vec![], Some(Grid::filled(shaped.spec, 0.0))),
            (&flat, vec![], None),
        ] {
            let out = run(node, g, &params, mask, None);
            assert_eq!(out["height"].grid().data, g.data);
            for (key, v) in &out {
                if key != "height" && key != "flow" {
                    assert!(v.grid().data.iter().all(|v| *v == 0.0), "{key}");
                }
            }
        }
    }
}
#[test]
fn local_mask_protects_cells_and_hardness_reduces_wear() {
    let g = terrain(65);
    let mask = Grid::from_fn(g.spec, |x, _| if x < 512.0 { 0.0 } else { 1.0 });
    for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
        let masked = run(node, &g, &[], Some(mask.clone()), None);
        assert!(
            masked["height"]
                .grid()
                .data
                .iter()
                .zip(&g.data)
                .zip(&mask.data)
                .all(|((a, b), m)| *m > 0.0 || a == b)
        );
        assert_ne!(masked["height"].grid().data, g.data);
        let hard = run(node, &g, &[], None, Some(Grid::filled(g.spec, 1.0)));
        assert_eq!(hard["height"].grid().data, g.data);
    }
    let soft = run(&Hydraulic::default(), &g, &[], None, None);
    let hard = run(
        &Hydraulic::default(),
        &g,
        &[],
        None,
        Some(Grid::filled(g.spec, 0.9)),
    );
    assert!(hard["wear"].grid().mean() < soft["wear"].grid().mean());
}
/// Slopes are worn down and, where the river can't carry everything (a
/// gentle valley floor), the sediment settles there; all masks stay in 0..1.
#[test]
fn hydraulic_wears_slopes_and_fills_a_gentle_valley_floor() {
    // A V-shaped valley along y, draining towards the y = 0 edge, with bumps.
    let spec = GridSpec::full_world(&world(), 129).unwrap();
    let g = Grid::from_fn(spec, |x, y| {
        (200.0 + (x - 512.0).abs() * 0.3 + y * 0.1 + 6.0 * libm::sin(x / 37.0) * libm::cos(y / 29.0)) as f32
    });
    let out = run(&Hydraulic::default(), &g, &[], None, None);
    for port in ["flow", "wear", "deposition", "sediment"] {
        let values = &out[port].grid().data;
        assert!(
            values.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "{port}"
        );
        assert!(values.iter().any(|v| *v > 0.001), "{port} has no signal");
    }
    let h = out["height"].grid();
    let lowered = |i: u32, j: u32| g.get(i, j) - h.get(i, j);
    let flank = (40..90).map(|j| lowered(24, j)).sum::<f32>() / 50.0;
    assert!(flank > 1.0, "flank lowered by {flank} m on average");
    let dep = out["deposition"].grid();
    let floor_dep = (40..90).map(|j| dep.get(64, j)).sum::<f32>();
    let flank_dep = (40..90).map(|j| dep.get(24, j)).sum::<f32>();
    assert!(
        floor_dep > flank_dep,
        "deposition floor {floor_dep}, flank {flank_dep}"
    );
}
#[test]
fn thermal_conserves_mass_and_reduces_steep_slopes() {
    let g = terrain(65);
    let out = run(&Thermal::default(), &g, &[], None, None);
    let h = out["height"].grid();
    assert!((h.mean() - g.mean()).abs() < 0.002);
    let steepness = |g: &Grid| -> f64 {
        g.data
            .chunks(g.spec.width as usize)
            .flat_map(|row| row.windows(2))
            .map(|p| {
                ((p[1] - p[0]).abs() as f64 / g.spec.cell_size_m()[0] - libm::tan(35.0_f64.to_radians()))
                    .max(0.0)
            })
            .sum()
    };
    assert!(steepness(h) < steepness(&g));
    assert!(out["debris"].grid().data.iter().any(|v| *v > 0.01));
}
/// A sand pile relaxes to the talus angle in every direction, not only along
/// the grid axes (a four-neighbour stencil leaves diagonals near 45°).
#[test]
fn thermal_talus_angle_is_the_same_on_diagonals() {
    let spec = GridSpec {
        width: 61,
        height: 61,
        origin_m: [0.0, 0.0],
        extent_m: [60.0, 60.0],
        window: None,
    };
    let c = 30i64;
    let pile = Grid::from_fn_indexed(spec, |i, _, _| {
        let (x, y) = ((i % 61) as i64, (i / 61) as i64);
        if (x - c).abs() <= 1 && (y - c).abs() <= 1 {
            40.0
        } else {
            0.0
        }
    });
    let out = run(
        &Thermal::default(),
        &pile,
        &[("duration_s", 600.0), ("diffusivity_m2_s", 10.0)],
        None,
        None,
    );
    let h = out["height"].grid();
    let at = |x: i64, y: i64| h.get(x as u32, y as u32) as f64;
    let apex = at(c, c);
    let r = 6;
    let axis = (at(c - r, c) + at(c + r, c) + at(c, c - r) + at(c, c + r)) / 4.0;
    let q = 4; // 4·√2 ≈ 5.7 m out
    let diag = (at(c - q, c - q) + at(c + q, c + q) + at(c - q, c + q) + at(c + q, c - q)) / 4.0;
    let axis_deg = ((apex - axis) / r as f64).atan().to_degrees();
    let diag_deg = ((apex - diag) / (q as f64 * std::f64::consts::SQRT_2))
        .atan()
        .to_degrees();
    println!("apex {apex:.2} m, axis {axis_deg:.1}°, diagonal {diag_deg:.1}°");
    assert!(
        axis > 0.0 && diag > 0.0,
        "the pile must spread past the sample points"
    );
    assert!(
        axis_deg < 36.0 && diag_deg < 36.0,
        "axis {axis_deg:.1}°, diagonal {diag_deg:.1}°"
    );
    assert!(
        (axis_deg - diag_deg).abs() < 3.0,
        "axis {axis_deg:.1}°, diagonal {diag_deg:.1}°"
    );
    let total = |g: &Grid| g.data.iter().map(|&v| v as f64).sum::<f64>();
    assert!((total(h) - total(&pile)).abs() < 0.01 * total(&pile));
}

#[test]
fn cancellation_during_simulation_and_monotonic_progress() {
    let g = terrain(65);
    let world = world();
    for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
        let params = BTreeMap::new();
        let cancelled = AtomicBool::new(false);
        let samples = Mutex::new(Vec::new());
        let callback = |p| {
            samples.lock().unwrap().push(p);
            if p >= 0.3 {
                cancelled.store(true, Ordering::Relaxed);
            }
        };
        let ctx = EvalContext::new(
            &world,
            g.spec,
            0,
            "erosion",
            node.schema(),
            &params,
            BTreeMap::from([("in".into(), Value::Heightfield(Arc::new(g.clone())))]),
        )
        .with_controls(Some(&cancelled), Some(&callback));
        assert!(matches!(node.evaluate(&ctx), Err(CoreError::Cancelled)));
        let samples = samples.into_inner().unwrap();
        assert!(samples.len() > 2);
        assert!(samples.windows(2).all(|p| p[1] >= p[0]));
        assert!(*samples.last().unwrap() < 1.0);
    }
}
#[test]
fn rectangular_grid_and_invalid_inputs() {
    let spec = GridSpec {
        width: 17,
        height: 9,
        origin_m: [-50.0, 12.0],
        extent_m: [100.0, 70.0],
        window: None,
    };
    let g = Grid::from_fn(spec, |x, y| (x + y) as f32);
    for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
        let out = run(node, &g, &[("duration_s", 2.0)], None, None);
        assert!(
            out.values()
                .all(|v| v.grid().spec == spec && v.grid().data.iter().all(|v| v.is_finite()))
        );
        let world = world();
        let params = BTreeMap::new();
        let mut bad = g.clone();
        bad.data[0] = f32::NAN;
        let ctx = EvalContext::new(
            &world,
            spec,
            0,
            "bad",
            node.schema(),
            &params,
            BTreeMap::from([("in".into(), Value::Heightfield(Arc::new(bad)))]),
        );
        assert!(node.evaluate(&ctx).is_err());
    }
}

#[test]
fn hardness_beds_use_world_heights_and_drive_erosion_through_graph() {
    use terrain_core::{EvalOptions, Project, evaluate_node};
    use terrain_nodes::erosion::RockHardness;
    let node = RockHardness::default();
    let g = Grid::from_fn(terrain(33).spec, |x, _| (x * 0.5 - 100.0) as f32);
    let out = run(&node, &g, &[], None, None);
    assert!(out["out"].grid().data.iter().all(|v| (0.1..=0.9).contains(v)));
    assert!(out["out"].grid().min_max().1 - out["out"].grid().min_max().0 > 0.7);
    let reg = terrain_nodes::registry();
    let mut p = Project::default();
    let src = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let hard = p.graph.add_node(&reg, "data.rock_hardness", [0.0, 0.0]).unwrap();
    let hydraulic = p.graph.add_node(&reg, "simulate.hydraulic", [0.0, 0.0]).unwrap();
    let thermal = p.graph.add_node(&reg, "simulate.thermal", [0.0, 0.0]).unwrap();
    for (a, ap, b, bp) in [
        (&src, "out", &hard, "in"),
        (&src, "out", &hydraulic, "in"),
        (&hard, "out", &hydraulic, "hardness"),
        (&hydraulic, "height", &thermal, "in"),
        (&hydraulic, "flow", &thermal, "mask"),
    ] {
        p.graph.connect(&reg, a, ap, b, bp).unwrap();
    }
    let (p, warnings) = Project::from_json(&p.to_json().unwrap(), &reg).unwrap();
    assert!(warnings.is_empty());
    let outputs = evaluate_node(
        &p.graph,
        &reg,
        &p.world,
        GridSpec::full_world(&p.world, 65).unwrap(),
        &thermal,
        &EvalOptions::default(),
    )
    .unwrap();
    assert_eq!(outputs.len(), 2);
}

#[test]
fn erosion_masks_export_as_linear_16_bit_png_and_float_exr() {
    use terrain_core::export::{ExportFormat, ExportRequest, export_node};
    use terrain_core::{EvalOptions, Project, evaluate_node};
    let reg = terrain_nodes::registry();
    let mut p = Project::default();
    let source = p.graph.add_node(&reg, "noise.fbm", [0.0, 0.0]).unwrap();
    let node = p.graph.add_node(&reg, "simulate.hydraulic", [0.0, 0.0]).unwrap();
    p.graph.connect(&reg, &source, "out", &node, "in").unwrap();
    let expected = evaluate_node(
        &p.graph,
        &reg,
        &p.world,
        GridSpec::full_world(&p.world, 33).unwrap(),
        &node,
        &EvalOptions::default(),
    )
    .unwrap();
    let folder = std::env::temp_dir().join(format!("ots-erosion-export-{}", std::process::id()));
    for port in ["flow", "wear", "deposition", "sediment"] {
        let files = export_node(
            &p,
            &reg,
            &ExportRequest {
                node: &node,
                port,
                resolution: 33,
                folder: &folder,
                formats: &[ExportFormat::Png16, ExportFormat::Exr32],
            },
            &EvalOptions::default(),
        )
        .unwrap();
        let png = image::open(files.iter().find(|p| p.extension().unwrap() == "png").unwrap()).unwrap();
        assert_eq!(png.color(), image::ColorType::L16);
        for (actual, expected) in png.into_luma16().as_raw().iter().zip(&expected[port].grid().data) {
            assert!((*actual as f32 / 65535.0 - expected).abs() <= 1.0 / 65535.0);
        }
        let exr = exr::prelude::read_first_flat_layer_from_file(
            files.iter().find(|p| p.extension().unwrap() == "exr").unwrap(),
        )
        .unwrap();
        let values: Vec<f32> = exr.layer_data.channel_data.list[0]
            .sample_data
            .values_as_f32()
            .collect();
        assert_eq!(values, expected[port].grid().data);
    }
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn shipped_erosion_project_loads_and_its_marked_outputs_exist() {
    use terrain_core::{EvalOptions, Project, evaluate_node};
    let reg = terrain_nodes::registry();
    let (p, warnings) =
        Project::from_json(include_str!("../../../app/examples/eroded_strata.otstudio"), &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for mark in &p.exports {
        let node = p.graph.node(&mark.node).unwrap();
        assert!(reg.schema(&node.type_id).unwrap().output(&mark.port).is_some());
    }
    let target = p.ui["viewed_node"].as_str().unwrap();
    let out = evaluate_node(
        &p.graph,
        &reg,
        &p.world,
        GridSpec::full_world(&p.world, 65).unwrap(),
        target,
        &EvalOptions::default(),
    )
    .unwrap();
    assert!(out["height"].grid().data.iter().all(|v| v.is_finite()));
}

/// Flow is the area draining through each cell: dark on crests, bright in
/// the channel where the whole valley gathers, never saturated.
#[test]
fn flow_is_dark_on_crests_and_bright_in_channels() {
    let spec = GridSpec::full_world(&world(), 129).unwrap();
    let g = Grid::from_fn(spec, |x, y| (200.0 + (x - 512.0).abs() * 0.3 + y * 0.1) as f32);
    let out = run(&Hydraulic::default(), &g, &[], None, None);
    let flow = out["flow"].grid();
    let channel = (0..129).map(|i| flow.get(i, 16)).fold(0.0f32, f32::max);
    let (crest_l, crest_r) = (flow.get(2, 64), flow.get(126, 64));
    assert!(channel > 0.25, "channel {channel}");
    assert!(crest_l < 0.1 && crest_r < 0.1, "crests {crest_l} {crest_r}");
    assert!(flow.data.iter().all(|&v| v < 0.95), "flow saturated");
}
