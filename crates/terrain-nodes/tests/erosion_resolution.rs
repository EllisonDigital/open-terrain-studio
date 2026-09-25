//! Actual 512² / 2048² regression, including erosion deltas rather than only
//! the much larger input height signal. Runs in the normal CI test suite.
use std::collections::BTreeMap;
use std::sync::Arc;
use terrain_core::{EvalContext, Grid, GridSpec, NodeKind, Outputs, ParamValue, Value, World};
use terrain_nodes::erosion::{Hydraulic, Thermal};

fn run(node: &dyn NodeKind, resolution: u32, foothill: bool) -> (Grid, Outputs) {
    let world = World {
        size_m: [4096.0, 4096.0],
        ..World::default()
    };
    let spec = GridSpec::full_world(&world, resolution).unwrap();
    let input = Grid::from_fn(spec, |x, y| {
        let ridge = if foothill {
            let distance = libm::sqrt((x - 2048.0).powi(2) + 40.0_f64.powi(2));
            let ramp = 600.0 - 1.5 * distance;
            0.5 * (ramp + libm::sqrt(ramp * ramp + 50.0_f64.powi(2)))
        } else {
            600.0 * libm::exp(-((x - 2048.0) / 400.0).powi(2))
        };
        (100.0 + ridge + 0.12 * y + 60.0 * libm::sin(x / 300.0) * libm::cos(y / 400.0)) as f32
    });
    let params = BTreeMap::from([("duration_s".into(), ParamValue::Float(12.0))]);
    let outputs = node
        .evaluate(&EvalContext::new(
            &world,
            spec,
            1,
            "erosion",
            node.schema(),
            &params,
            BTreeMap::from([("in".into(), Value::Heightfield(Arc::new(input.clone())))]),
        ))
        .unwrap();
    (input, outputs)
}

#[test]
fn erosion_at_512_and_2048_preserves_effect_and_masks() {
    // Avoid oversubscribing a shared CI runner with its reported host CPU count.
    rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap().install(|| {
        for foothill in [false, true] {
        for node in [&Hydraulic::default() as &dyn NodeKind, &Thermal::default()] {
            let (low_input, low) = run(node, 512, foothill);
            let (high_input, high) = run(node, 2048, foothill);
            let delta = |input: &Grid, out: &Outputs| Grid { spec: input.spec,
                data: out["height"].grid().data.iter().zip(&input.data).map(|(a,b)| a-b).collect() };
            let low_delta = delta(&low_input, &low);
            let high_delta = delta(&high_input, &high);
            let mut error = 0.0f64;
            let mut energy = 0.0f64;
            let mut dot = 0.0f64;
            let mut high_energy = 0.0f64;
            let mut samples = 0usize;
            for y in 8..504 { for x in 8..504 {
                let a = low_delta.get(x,y) as f64;
                let b = high_delta.sample_bilinear_m(low_input.spec.x_m(x),low_input.spec.y_m(y)) as f64;
                error += (a-b)*(a-b); energy += a*a; dot += a*b; high_energy += b*b; samples += 1;
            }}
            let rms_effect = (energy / samples as f64).sqrt();
            let relative_error = (error / energy).sqrt();
            let similarity = dot / (energy*high_energy).sqrt();
            println!("{}: foothill={foothill}: delta RMS {rms_effect:.6}m, relative RMSE {relative_error:.4}, cosine {similarity:.4}", node.schema().type_id);
            assert!(rms_effect > 0.005, "test must exercise measurable erosion");
            assert!(relative_error < 0.1, "erosion delta changes with resolution: {relative_error}");
            assert!(similarity > 0.99, "erosion appears in different places: {similarity}");
            for (key, value) in &low {
                if key == "height" { continue; }
                let g = value.grid(); let h = high[key].grid();
                let mut error = 0.0f64; let mut energy = 0.0f64;
                for y in 8..504 { for x in 8..504 {
                    let a = g.get(x,y) as f64;
                    let b = h.sample_bilinear_m(g.spec.x_m(x),g.spec.y_m(y)) as f64;
                    error += (a-b)*(a-b); energy += a*a;
                }}
                let rms = (error / samples as f64).sqrt();
                println!("  {key}: mask RMSE {rms:.6}, relative {:.4}", (error/energy.max(1.0e-20)).sqrt());
                if foothill && key == "deposition" { assert!(energy / samples as f64 > 1.0e-6, "foothill must deposit sediment"); }
                // Allow two percent absolute mask error (about five 8-bit levels),
                // and separately constrain relative error for non-negligible masks.
                if energy / samples as f64 > 1.0e-6 {
                    assert!((error / energy).sqrt() < 0.15, "{key} relative mask error");
                }
                assert!(rms < 0.02, "{key} mask changed with resolution: {rms}");
            }
        }
        }
    });
}
