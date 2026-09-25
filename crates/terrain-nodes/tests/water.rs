//! Water and hydrology nodes: behaviour on known shapes, every output in
//! range and deterministic across thread counts.

use std::collections::BTreeMap;
use std::sync::Arc;

use terrain_core::{EvalContext, Grid, GridSpec, NodeKind, Outputs, ParamValue, Value, World};
use terrain_nodes::water::{Flow, Lakes, Rivers};

fn world() -> World {
    World {
        size_m: [1024.0, 1024.0],
        ..World::default()
    }
}

fn spec(res: u32) -> GridSpec {
    GridSpec::full_world(&world(), res).unwrap()
}

/// Two V-shaped valleys running along y, split by a ridge at x = 512 and
/// draining towards the y = 0 edge.
fn two_valleys(res: u32) -> Grid {
    Grid::from_fn(spec(res), |x, y| {
        let valley = ((x - 256.0).abs()).min((x - 768.0).abs());
        (200.0 + valley * 0.3 + y * 0.1 + 3.0 * libm::sin(x / 37.0) * libm::cos(y / 29.0)) as f32
    })
}

fn run(node: &dyn NodeKind, grid: &Grid, params: &[(&str, ParamValue)]) -> Outputs {
    let world = world();
    let params = params.iter().map(|(k, v)| ((*k).into(), v.clone())).collect();
    let inputs = BTreeMap::from([("in".into(), Value::Heightfield(Arc::new(grid.clone())))]);
    node.evaluate(&EvalContext::new(
        &world,
        grid.spec,
        7,
        "water",
        node.schema(),
        &params,
        inputs,
    ))
    .unwrap()
}

fn bits(g: &Grid) -> Vec<u32> {
    g.data.iter().map(|v| v.to_bits()).collect()
}

fn water_nodes() -> Vec<Box<dyn NodeKind>> {
    vec![
        Box::new(Flow::default()),
        Box::new(Lakes::default()),
        Box::new(Rivers::default()),
    ]
}

#[test]
fn every_output_is_a_finite_normalised_mask_or_heightfield_and_deterministic() {
    let g = two_valleys(65);
    for node in water_nodes() {
        let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let many = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        let a = one.install(|| run(node.as_ref(), &g, &[]));
        let b = many.install(|| run(node.as_ref(), &g, &[]));
        let id = &node.schema().type_id;
        for port in &node.schema().outputs {
            let v = &a[&port.key];
            assert_eq!(v.port_type(), port.ty, "{id}.{}", port.key);
            let data = &v.grid().data;
            assert!(data.iter().all(|v| v.is_finite()), "{id}.{} not finite", port.key);
            if port.ty == terrain_core::PortType::Mask {
                assert!(
                    data.iter().all(|v| (0.0..=1.0).contains(v)),
                    "{id}.{} outside 0..1",
                    port.key
                );
            }
            assert_eq!(
                bits(v.grid()),
                bits(b[&port.key].grid()),
                "{id}.{} thread counts",
                port.key
            );
        }
    }
}

#[test]
fn flow_is_bright_in_valleys_dark_on_ridges_and_points_downhill() {
    let g = two_valleys(129);
    for method in ["dinf", "d8"] {
        let out = run(
            &Flow::default(),
            &g,
            &[
                ("method", ParamValue::Text(method.into())),
                ("detail_m", ParamValue::Float(8.0)),
            ],
        );
        let flow = out["accumulation"].grid();
        // Column 32 is x = 256 (a valley floor), column 64 the ridge at x = 512.
        // The stream wanders a few metres around the floor's centre line.
        let peak = |j: u32, i: u32| (i - 4..=i + 4).map(|i| flow.get(i, j)).fold(0.0f32, f32::max);
        let floor = (10..60).map(|j| peak(j, 32)).sum::<f32>() / 50.0;
        let ridge = (10..60).map(|j| flow.get(64, j)).sum::<f32>() / 50.0;
        assert!(floor > ridge + 0.3, "{method}: floor {floor}, ridge {ridge}");
        // Flow increases downstream (towards y = 0) along the valley floor.
        assert!(peak(20, 32) > peak(100, 32), "{method}");

        // Down the valley floor water runs towards -y: about 270°.
        let dir = out["direction"].grid();
        let deg = dir.get(32, 60) * 360.0;
        assert!((deg - 270.0).abs() < 50.0, "{method}: floor direction {deg}°");
        // On the west flank of the east valley water runs towards +x (0°/360°).
        let deg = dir.get(80, 60) * 360.0;
        assert!(!(60.0..300.0).contains(&deg), "{method}: flank direction {deg}°");
    }
}

#[test]
fn separate_valleys_are_separate_basins() {
    let g = two_valleys(129);
    let out = run(&Flow::default(), &g, &[("detail_m", ParamValue::Float(8.0))]);
    let basins = out["basins"].grid();
    let west = basins.get(32, 80);
    assert_eq!(west, basins.get(28, 110), "one valley, one basin");
    assert_ne!(west, basins.get(96, 80), "two valleys, two basins");
}

#[test]
fn a_closed_pit_fills_and_spills_instead_of_stopping_the_flow() {
    // A bowl in the middle of a slope: water entering it still reaches the edge.
    let g = Grid::from_fn(spec(97), |x, y| {
        let bowl = 40.0 * libm::exp(-(((x - 512.0) / 120.0).powi(2) + ((y - 512.0) / 120.0).powi(2)));
        (300.0 + y * 0.2 - bowl) as f32
    });
    let out = run(&Flow::default(), &g, &[("detail_m", ParamValue::Float(8.0))]);
    let flow = out["accumulation"].grid();
    // Below the bowl (towards y = 0) the flow carries the bowl's catchment.
    let below = (44..53).map(|i| flow.get(i, 20)).fold(0.0f32, f32::max);
    let beside = (4..13).map(|i| flow.get(i, 20)).fold(0.0f32, f32::max);
    assert!(below > beside, "below {below}, beside {beside}");
}

/// A round bowl in gently sloping ground: its rim is lowest on the downhill
/// (y = 0) side, where the lake spills.
fn bowl(res: u32) -> Grid {
    Grid::from_fn(spec(res), |x, y| {
        let r2 = ((x - 512.0) / 150.0).powi(2) + ((y - 512.0) / 150.0).powi(2);
        (300.0 + y * 0.05 - 40.0 * libm::exp(-r2)) as f32
    })
}

#[test]
fn a_hollow_fills_to_its_spill_level_with_a_flat_surface() {
    let g = bowl(129);
    let out = run(&Lakes::default(), &g, &[("detail_m", ParamValue::Float(8.0))]);
    let lakes = out["lakes"].grid();
    let surface = out["water_surface"].grid();
    let bed = out["height"].grid();
    // The centre is water, far corners are dry.
    assert_eq!(lakes.get(64, 64), 1.0);
    assert_eq!(lakes.get(5, 120), 0.0);
    // One flat level across the lake, above the original ground and bed.
    let level = surface.get(64, 64);
    for (i, j) in [(60, 60), (70, 66), (64, 72)] {
        assert_eq!(surface.get(i, j), level);
        assert!(g.get(i, j) < level && bed.get(i, j) < level);
    }
    // Sediment raised the deepest point, never lowered anything.
    assert!(bed.get(64, 64) > g.get(64, 64));
    assert!(bed.data.iter().zip(&g.data).all(|(b, h)| b >= h));
    // Dry ground is unchanged and its surface is the ground.
    assert_eq!(bed.get(5, 120), g.get(5, 120));
    assert_eq!(surface.get(5, 120), g.get(5, 120));
    // The shore is brightest around the waterline, dark far from it.
    let shore = out["shore"].grid();
    assert_eq!(shore.get(5, 120), 0.0);
    assert!(shore.data.iter().any(|v| *v > 0.8));
}

#[test]
fn small_or_shallow_hollows_stay_dry_and_edge_hollows_drain() {
    let g = bowl(129);
    for params in [
        vec![("min_area_m2", ParamValue::Float(1.0e7))],
        vec![("min_depth_m", ParamValue::Float(100.0))],
    ] {
        let out = run(&Lakes::default(), &g, &params);
        assert!(out["lakes"].grid().data.iter().all(|v| *v == 0.0));
        assert_eq!(out["height"].grid().data, g.data);
    }
    // A hollow cut by the world's edge is open: water leaves.
    let open = Grid::from_fn(spec(129), |x, y| {
        let r2 = ((x - 0.0) / 150.0).powi(2) + ((y - 512.0) / 150.0).powi(2);
        (300.0 - 40.0 * libm::exp(-r2)) as f32
    });
    let out = run(&Lakes::default(), &open, &[]);
    assert!(out["lakes"].grid().data.iter().all(|v| *v == 0.0));
}

#[test]
fn rivers_carve_valley_floors_down_to_the_edge_and_leave_ridges() {
    let g = two_valleys(129);
    let out = run(
        &Rivers::default(),
        &g,
        &[
            ("source_area_km2", ParamValue::Float(0.02)),
            ("detail_m", ParamValue::Float(8.0)),
        ],
    );
    let (h, river, bank, surface) = (
        out["height"].grid(),
        out["river"].grid(),
        out["riverbank"].grid(),
        out["water_surface"].grid(),
    );
    // Along the west valley floor, from the headwaters to the y = 0 edge.
    for j in [1, 20, 60] {
        let (i, wet) = (26..=38)
            .map(|i| (i, river.get(i, j)))
            .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
        assert!(wet > 0.9, "row {j}: no river (max {wet})");
        assert!(h.get(i, j) < g.get(i, j) - 0.5, "row {j}: not carved");
        assert!(surface.get(i, j) > h.get(i, j), "row {j}: no water above the bed");
    }
    // The ridge between the valleys is untouched and dry.
    for j in [20, 60, 100] {
        assert_eq!(h.get(64, j), g.get(64, j));
        assert_eq!(river.get(64, j), 0.0);
        assert_eq!(surface.get(64, j), g.get(64, j));
    }
    // Carving only lowers; banks sit beside the water, not in it.
    assert!(h.data.iter().zip(&g.data).all(|(a, b)| a <= b));
    assert!(bank.data.iter().any(|v| *v > 0.5));
    assert!(
        bank.data
            .iter()
            .zip(&river.data)
            .all(|(b, r)| *r < 1.0 || *b == 0.0)
    );
}

#[test]
fn outputs_match_across_resolutions() {
    // 129 and 513 samples: every 4th fine sample sits on a coarse one.
    let params = [
        ("detail_m", ParamValue::Float(16.0)),
        ("source_area_km2", ParamValue::Float(0.02)),
    ];
    for node in water_nodes() {
        for terrain in [two_valleys as fn(u32) -> Grid, bowl] {
            let lo = run(node.as_ref(), &terrain(129), &params);
            let hi = run(node.as_ref(), &terrain(513), &params);
            for (key, v) in &lo {
                let (a, b) = (v.grid(), hi[key].grid());
                let (min, max) = b.min_max();
                let range = ((max - min) as f64).max(1.0e-3);
                let mut diffs: Vec<f64> = (0..129u32)
                    .flat_map(|j| (0..129u32).map(move |i| (i, j)))
                    .map(|(i, j)| (a.get(i, j) - b.get(i * 4, j * 4)).abs() as f64 / range)
                    .collect();
                diffs.sort_by(f64::total_cmp);
                let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
                let p99 = diffs[diffs.len() * 99 / 100];
                let id = &node.schema().type_id;
                // Shore bands come from a distance transform, exact to about
                // one coarse cell (8 m of a 30 m band).
                let tol_p99 = if key == "shore" { 0.3 } else { 0.1 };
                assert!(
                    mean < 0.01 && p99 < tol_p99,
                    "{id}.{key}: mean {mean:.4}, p99 {p99:.4}"
                );
            }
        }
    }
}
