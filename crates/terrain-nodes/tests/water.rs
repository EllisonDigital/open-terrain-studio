//! Water and hydrology nodes: behaviour on known shapes, every output in
//! range and deterministic across thread counts.

use std::collections::BTreeMap;
use std::sync::Arc;

use terrain_core::{EvalContext, Grid, GridSpec, NodeKind, Outputs, ParamValue, Value, World};
use terrain_nodes::water::Flow;

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
    vec![Box::new(Flow::default())]
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
        let floor = (10..60).map(|j| flow.get(32, j)).sum::<f32>() / 50.0;
        let ridge = (10..60).map(|j| flow.get(64, j)).sum::<f32>() / 50.0;
        assert!(floor > ridge + 0.3, "{method}: floor {floor}, ridge {ridge}");
        // Flow increases downstream (towards y = 0) along the valley floor.
        assert!(flow.get(32, 20) > flow.get(32, 100), "{method}");

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
