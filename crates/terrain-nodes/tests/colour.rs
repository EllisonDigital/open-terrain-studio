//! Colour and texturing nodes on known inputs, and colour export/import.

use std::collections::BTreeMap;
use std::sync::Arc;

use terrain_core::export::{ExportFormat, write_value};
use terrain_core::{ColorGrid, EvalContext, Grid, GridSpec, NodeKind, Outputs, ParamValue, Value, World};
use terrain_nodes::colour::{Blend, Colourise, Image, Layers, NormalMap, Occlusion, Splat};

fn world() -> World {
    World {
        size_m: [1024.0, 1024.0],
        ..World::default()
    }
}

fn spec(res: u32) -> GridSpec {
    GridSpec::full_world(&world(), res).unwrap()
}

fn run(node: &dyn NodeKind, inputs: Vec<(&str, Value)>, params: &[(&str, ParamValue)]) -> Outputs {
    run_in(node, inputs, params, None)
}

fn run_in(
    node: &dyn NodeKind,
    inputs: Vec<(&str, Value)>,
    params: &[(&str, ParamValue)],
    base_dir: Option<&std::path::Path>,
) -> Outputs {
    let world = world();
    let spec = inputs.first().map_or(spec(17), |(_, v)| v.spec());
    let params: BTreeMap<String, ParamValue> = params.iter().map(|(k, v)| ((*k).into(), v.clone())).collect();
    let inputs = inputs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let mut ctx = EvalContext::new(&world, spec, 7, "colour", node.schema(), &params, inputs);
    ctx.base_dir = base_dir;
    node.evaluate(&ctx).unwrap()
}

fn mask(g: Grid) -> Value {
    Value::Mask(Arc::new(g))
}

fn solid(spec: GridSpec, c: [f32; 4]) -> Value {
    Value::ColorMap(Arc::new(ColorGrid::from_fn_indexed(spec, |_, _, _| c)))
}

fn text(s: &str) -> ParamValue {
    ParamValue::Text(s.into())
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1.0e-4)
}

#[test]
fn colourise_maps_the_input_through_the_gradient() {
    let ramp = Grid::from_fn(spec(33), |x, _| (x / 1024.0) as f32);
    let custom = ParamValue::Other(serde_json::json!([[0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]]));
    let out = run(
        &Colourise::default(),
        vec![("in", mask(ramp.clone()))],
        &[("preset", text("custom")), ("gradient", custom.clone())],
    );
    let c = out["out"].color().unwrap();
    assert!(close(c.at(0), [1.0, 0.0, 0.0, 1.0]), "left: {:?}", c.at(0));
    assert!(close(c.at(32), [0.0, 0.0, 1.0, 1.0]), "right: {:?}", c.at(32));
    assert!(close(c.at(16), [0.5, 0.0, 0.5, 1.0]), "middle: {:?}", c.at(16));
    // Low/high stretch part of the input over the whole gradient.
    let out = run(
        &Colourise::default(),
        vec![("in", mask(ramp))],
        &[
            ("preset", text("custom")),
            ("gradient", custom),
            ("low", ParamValue::Float(0.25)),
            ("high", ParamValue::Float(0.75)),
        ],
    );
    let c = out["out"].color().unwrap();
    assert!(close(c.at(8), [1.0, 0.0, 0.0, 1.0]) && close(c.at(24), [0.0, 0.0, 1.0, 1.0]));
    // Every built-in gradient gives valid colours.
    for (key, _, _) in terrain_nodes::colour::GRADIENTS {
        let out = run(
            &Colourise::default(),
            vec![("in", mask(Grid::from_fn(spec(9), |x, _| (x / 1024.0) as f32)))],
            &[("preset", text(key))],
        );
        assert!(
            out["out"].samples().iter().all(|v| (0.0..=1.0).contains(v)),
            "{key}"
        );
    }
}

#[test]
fn blend_and_layers_paint_where_their_masks_are_white() {
    let s = spec(9);
    let (grey, red) = (solid(s, [0.5, 0.5, 0.5, 1.0]), solid(s, [1.0, 0.0, 0.0, 1.0]));
    let half = mask(Grid::from_fn(s, |x, _| if x < 512.0 { 0.0 } else { 1.0 }));
    let out = run(
        &Blend::default(),
        vec![("a", grey.clone()), ("b", red.clone()), ("mask", half.clone())],
        &[],
    );
    let c = out["out"].color().unwrap();
    assert!(close(c.at(0), [0.5, 0.5, 0.5, 1.0]) && close(c.at(8), [1.0, 0.0, 0.0, 1.0]));
    let out = run(
        &Blend::default(),
        vec![("a", grey.clone()), ("b", red.clone())],
        &[("mode", text("multiply"))],
    );
    assert!(close(out["out"].color().unwrap().at(4), [0.5, 0.0, 0.0, 1.0]));
    let out = run(
        &Blend::default(),
        vec![("a", grey.clone()), ("b", red.clone())],
        &[("opacity", ParamValue::Float(0.5))],
    );
    assert!(close(out["out"].color().unwrap().at(4), [0.75, 0.25, 0.25, 1.0]));

    // Layer 2 (blue, everywhere) is above layer 1 (red, right half).
    let blue = solid(s, [0.0, 0.0, 1.0, 1.0]);
    let out = run(
        &Layers::default(),
        vec![("base", grey.clone()), ("color_1", red), ("mask_1", half.clone())],
        &[],
    );
    let c = out["out"].color().unwrap();
    assert!(close(c.at(0), [0.5, 0.5, 0.5, 1.0]) && close(c.at(8), [1.0, 0.0, 0.0, 1.0]));
    let out = run(&Layers::default(), vec![("base", grey), ("color_2", blue)], &[]);
    assert!(close(out["out"].color().unwrap().at(0), [0.0, 0.0, 1.0, 1.0]));
}

#[test]
fn normal_maps_point_away_from_slopes_in_both_conventions() {
    let s = spec(33);
    let flat = Value::Heightfield(Arc::new(Grid::filled(s, 100.0)));
    let out = run(&NormalMap::default(), vec![("in", flat)], &[]);
    assert!(close(out["out"].color().unwrap().at(100), [0.5, 0.5, 1.0, 1.0]));
    // Ground rising towards +X faces -X: red below 0.5.
    let east = Value::Heightfield(Arc::new(Grid::from_fn(s, |x, _| x as f32)));
    let c = run(&NormalMap::default(), vec![("in", east)], &[])["out"]
        .color()
        .unwrap()
        .at(100);
    assert!(c[0] < 0.2 && (c[1] - 0.5).abs() < 1.0e-4 && (c[0] - 0.5).powi(2) + (c[2] - 0.5).powi(2) > 0.2);
    // Ground rising towards +Y (down the image) faces up the image:
    // OpenGL green above 0.5, DirectX below.
    let south = Value::Heightfield(Arc::new(Grid::from_fn(s, |_, y| y as f32)));
    let gl = run(&NormalMap::default(), vec![("in", south.clone())], &[])["out"]
        .color()
        .unwrap()
        .at(100);
    let dx = run(
        &NormalMap::default(),
        vec![("in", south)],
        &[("convention", text("directx"))],
    )["out"]
        .color()
        .unwrap()
        .at(100);
    assert!(gl[1] > 0.8 && dx[1] < 0.2 && (gl[1] + dx[1] - 1.0).abs() < 1.0e-4);
}

#[test]
fn occlusion_darkens_hollows_and_leaves_open_ground_white() {
    let s = spec(65);
    let flat = Value::Heightfield(Arc::new(Grid::filled(s, 100.0)));
    let out = run(&Occlusion::default(), vec![("in", flat)], &[]);
    assert!(out["out"].grid().data.iter().all(|v| *v == 1.0));
    let pit = Grid::from_fn(s, |x, y| {
        (100.0 - 60.0 * libm::exp(-(((x - 512.0) / 60.0).powi(2) + ((y - 512.0) / 60.0).powi(2)))) as f32
    });
    let out = run(
        &Occlusion::default(),
        vec![("in", Value::Heightfield(Arc::new(pit)))],
        &[],
    );
    let o = out["out"].grid();
    assert!(o.get(32, 32) < 0.5, "pit {}", o.get(32, 32));
    assert_eq!(o.get(2, 2), 1.0);
}

#[test]
fn splat_weights_sum_to_one_and_the_base_fills_the_rest() {
    let s = spec(9);
    let quarter = mask(Grid::filled(s, 0.25));
    let half = mask(Grid::filled(s, 0.5));
    let out = run(
        &Splat::default(),
        vec![("layer_2", quarter.clone()), ("layer_3", half.clone())],
        &[],
    );
    let w = |k: usize| out[&format!("weight_{k}")].grid().get(4, 4);
    assert!((w(1) - 0.25).abs() < 1.0e-6 && (w(2) - 0.25).abs() < 1.0e-6 && (w(3) - 0.5).abs() < 1.0e-6);
    assert!(close(
        out["weights_1_4"].color().unwrap().at(40),
        [0.25, 0.25, 0.5, 0.0]
    ));
    assert!(close(out["weights_5_8"].color().unwrap().at(40), [0.0; 4]));
    // Overlapping masks are normalised.
    let full = mask(Grid::filled(s, 1.0));
    let out = run(
        &Splat::default(),
        vec![("layer_1", full.clone()), ("layer_8", full)],
        &[],
    );
    let (a, b) = (out["weight_1"].grid().get(0, 0), out["weight_8"].grid().get(0, 0));
    assert!((a - 0.5).abs() < 1.0e-6 && (b - 0.5).abs() < 1.0e-6);
    // Nothing connected: all base.
    let out = run(&Splat::default(), vec![], &[]);
    assert!(out["weight_1"].grid().data.iter().all(|v| *v == 1.0));
}

#[test]
fn colour_maps_export_as_rgba_and_import_back() {
    let dir = std::env::temp_dir().join(format!("ots-colour-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = spec(17);
    let colour = ColorGrid::from_fn_indexed(s, |i, x, _| [(x / 1024.0) as f32, 0.25, (i % 2) as f32, 1.0]);
    let value = Value::ColorMap(Arc::new(colour.clone()));
    for format in [ExportFormat::Png8, ExportFormat::Png16, ExportFormat::Exr32] {
        let path = dir.join(format!("colour.{}", format.extension()));
        let file = write_value(&value, &world(), format, &path, "n_0001", "out").unwrap();
        assert_eq!(file.data, terrain_core::PortType::ColorMap);
        // The Colour Image node reads it back onto the same grid.
        let out = run_in(
            &Image::default(),
            vec![],
            &[("path", ParamValue::Text(path.to_string_lossy().into_owned()))],
            None,
        );
        let back = out["out"].color().unwrap();
        let tol = if format == ExportFormat::Png8 {
            0.5 / 255.0 + 1.0e-6
        } else {
            1.0e-4
        };
        assert!(
            back.data
                .iter()
                .zip(&colour.data)
                .all(|(a, b)| (a - b).abs() <= tol),
            "{format:?} round trip"
        );
    }
    // 8-bit PNG really is 8-bit RGBA.
    let img = image::open(dir.join("colour.8bit.png")).unwrap();
    assert_eq!(img.color(), image::ColorType::Rgba8);
    let img = image::open(dir.join("colour.png")).unwrap();
    assert_eq!(img.color(), image::ColorType::Rgba16);
}
