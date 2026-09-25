//! GPU-vs-CPU tolerance checks for every kernel (ARCHITECTURE.md §6).
//!
//! Needs a real device, so it runs inside the app (`app/tests/gpu_test.gd`
//! calls it through `TerrainBuilder.run_gpu_check`) rather than under
//! `cargo test`. Each case evaluates a small graph twice, on the CPU and on
//! the GPU, and compares every output of the node under test.

use std::time::Instant;

use terrain_core::{EvalOptions, Gpu, GridSpec, NodeRegistry, ParamValue, Project, evaluate_node};

/// Result of one case.
#[derive(Clone, Debug)]
pub struct CheckReport {
    /// e.g. "noise.voronoi mode=cells".
    pub case: String,
    pub port: String,
    /// Largest difference, as a share of the CPU output's value range.
    pub max_error: f64,
    /// Share of samples differing by more than the tolerance.
    pub outliers: f64,
    /// The allowed largest difference and share of outliers.
    pub tolerance: (f64, f64),
    pub cpu_ms: f64,
    pub gpu_ms: f64,
    pub passed: bool,
    /// Set if the GPU run failed (and fell back to the CPU).
    pub error: Option<String>,
    /// Where the largest difference is, and both values there.
    pub worst: String,
}

/// Allowed (largest difference as a share of the value range, share of
/// samples allowed to exceed it). The architecture's target is 0.01% of the
/// range everywhere. Two known exceptions may exceed it in a few samples:
/// - Fractal noise: kernels compute lattice positions in f32, which grow with
///   every octave; beyond about 8 octaves the finest ones lose precision.
/// - Voronoi "cells" is discontinuous at cell borders: a sample within f32
///   rounding of a border can take the neighbouring cell's value.
fn tolerance(type_id: &str, case: &str) -> (f64, f64) {
    match type_id {
        "noise.fbm" | "noise.ridged" | "noise.billow" | "noise.domain_warp" => (1e-4, 1e-3),
        "noise.voronoi" if case.contains("cells") => (1e-4, 1e-3),
        _ => (1e-4, 0.0),
    }
}

/// Node cases: (type id, parameters to set, drive "height_m"/"strength" by a mask).
type Case = (
    &'static str,
    Vec<(&'static str, ParamValue)>,
    Option<&'static str>,
);

fn cases(reg: &NodeRegistry) -> Vec<Case> {
    let text = |s: &str| ParamValue::Text(s.into());
    let mut cases: Vec<Case> = Vec::new();
    for schema in reg.schemas().filter(|s| s.gpu) {
        let id: &'static str = Box::leak(schema.type_id.clone().into_boxed_str());
        cases.push((id, vec![], None));
    }
    cases.extend([
        ("noise.fbm", vec![("basis", text("simplex"))], None),
        ("noise.fbm", vec![("basis", text("value"))], None),
        ("noise.fbm", vec![], Some("height_m")),
        (
            "noise.ridged",
            vec![("basis", text("simplex")), ("octaves", ParamValue::Int(12))],
            None,
        ),
        ("noise.billow", vec![("lacunarity", ParamValue::Float(2.7))], None),
        (
            "noise.perlin",
            vec![
                ("offset_x_m", ParamValue::Float(250_000.0)),
                ("feature_size_m", ParamValue::Float(500.0)),
            ],
            None,
        ),
        ("noise.voronoi", vec![("mode", text("f1_inverted"))], None),
        (
            "noise.voronoi",
            vec![("mode", text("f2_f1")), ("jitter", ParamValue::Float(0.4))],
            None,
        ),
        ("noise.voronoi", vec![("mode", text("cells"))], None),
        ("adjust.blur", vec![("radius_m", ParamValue::Float(2000.0))], None),
        ("adjust.blur", vec![], Some("strength")),
        (
            "adjust.sharpen",
            vec![("radius_m", ParamValue::Float(3000.0))],
            Some("amount"),
        ),
        (
            "adjust.transform",
            vec![
                ("rotation_deg", ParamValue::Float(33.0)),
                ("scale", ParamValue::Float(0.7)),
                ("move_x_m", ParamValue::Float(900.0)),
                ("height_scale", ParamValue::Float(0.8)),
            ],
            None,
        ),
        ("adjust.warp", vec![], Some("strength_m")),
        ("data.curvature", vec![("mode", text("both"))], None),
        (
            "data.curvature",
            vec![("mode", text("concave")), ("invert", ParamValue::Bool(true))],
            None,
        ),
        ("data.slope", vec![("min_deg", ParamValue::Float(5.0))], None),
        (
            "simulate.thermal",
            vec![("talus_angle_deg", ParamValue::Float(10.0))],
            None,
        ),
    ]);
    cases
}

/// A project with one `type_id` node whose required inputs are a smooth fBm
/// (through a CPU-only Levels),
/// like the CPU test fixtures; optionally with `driven` exposed and fed a mask.
fn project_for(reg: &NodeRegistry, case: &Case) -> (Project, String) {
    let (type_id, params, driven) = case;
    let mut p = Project::default();
    let id = p.graph.add_node(reg, type_id, [0.0, 0.0]).expect("node type");
    let schema = reg.schema(type_id).expect("schema").clone();
    // Mountain has no GPU kernel, so both runs feed the node the same CPU
    // result: the check measures this kernel, not the error of its inputs.
    let source = p
        .graph
        .add_node(reg, "terrain.mountain", [0.0, 0.0])
        .expect("mountain");
    for input in schema.inputs.iter().filter(|i| !i.optional) {
        p.graph
            .connect(reg, &source, "out", &id, &input.key)
            .expect("link");
    }
    if schema.param("duration_s").is_some() {
        p.graph
            .set_param(reg, &id, "duration_s", ParamValue::Float(20.0))
            .expect("duration");
    }
    for (k, v) in params {
        p.graph.set_param(reg, &id, k, v.clone()).expect("case param");
    }
    if let Some(key) = driven {
        let mask = p.graph.add_node(reg, "noise.perlin", [0.0, 0.0]).expect("perlin");
        p.graph
            .set_param(reg, &mask, "height_m", ParamValue::Float(2000.0))
            .expect("mask height");
        p.graph.set_exposed(reg, &id, key, true).expect("expose");
        p.graph
            .connect(reg, &mask, "out", &id, &terrain_core::node::param_port_key(key))
            .expect("mask link");
    }
    (p, id)
}

/// Compare the GPU and CPU results of every case at `resolution`².
pub fn check_all(gpu: &Gpu, resolution: u32) -> Vec<CheckReport> {
    let reg = crate::registry();
    let mut reports = Vec::new();
    for case in cases(&reg) {
        let (project, id) = project_for(&reg, &case);
        let name = std::iter::once(case.0.to_string())
            .chain(case.1.iter().map(|(k, v)| format!("{k}={}", serde_json_like(v))))
            .chain(case.2.map(|k| format!("{k} driven")))
            .collect::<Vec<_>>()
            .join(" ");
        let spec = GridSpec::full_world(&project.world, resolution).expect("resolution");
        let run = |gpu: Option<&Gpu>| {
            let started = Instant::now();
            let out = evaluate_node(
                &project.graph,
                &reg,
                &project.world,
                spec,
                &id,
                &EvalOptions {
                    gpu,
                    ..Default::default()
                },
            )
            .expect("evaluation");
            // Read every output back inside the timing.
            for v in out.values() {
                v.grid();
            }
            (out, started.elapsed().as_secs_f64() * 1000.0)
        };
        let fallbacks = gpu.stats().fallbacks;
        let (cpu, cpu_ms) = run(None);
        let (on_gpu, gpu_ms) = run(Some(gpu));
        let stats = gpu.stats();
        let error = (stats.fallbacks > fallbacks).then(|| stats.last_error.unwrap_or_default());
        for (port, c) in &cpu {
            let g = on_gpu[port].grid();
            let c = c.grid();
            let (lo, hi) = c.min_max();
            let range = ((hi - lo) as f64).max(1e-3);
            let tol = tolerance(case.0, &name);
            let mut max_error = 0.0f64;
            let mut worst = String::new();
            let mut over = 0usize;
            for (k, (a, b)) in c.data.iter().zip(&g.data).enumerate() {
                let e = if a.is_finite() && b.is_finite() {
                    (a - b).abs() as f64 / range
                } else {
                    f64::INFINITY
                };
                if e > max_error {
                    max_error = e;
                    let w = spec.width as usize;
                    worst = format!("({}, {}): cpu {a}, gpu {b}", k % w, k / w);
                }
                if e > tol.0 {
                    over += 1;
                }
            }
            let outliers = over as f64 / c.data.len() as f64;
            // The few allowed outliers may still differ by at most the value range.
            let passed = error.is_none() && outliers <= tol.1 && max_error <= 1.0;
            reports.push(CheckReport {
                case: name.clone(),
                port: port.clone(),
                max_error,
                outliers,
                tolerance: tol,
                cpu_ms,
                gpu_ms,
                passed,
                error: error.clone(),
                worst,
            });
        }
    }
    reports
}

fn serde_json_like(v: &ParamValue) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => f.to_string(),
        ParamValue::Text(s) => s.clone(),
        ParamValue::Other(j) => j.to_string(),
    }
}
