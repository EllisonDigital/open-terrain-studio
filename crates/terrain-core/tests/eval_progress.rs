use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use terrain_core::error::{CoreError, Result};
use terrain_core::{
    EvalContext, EvalOptions, Grid, GridSpec, NodeKind, NodeRegistry, NodeSchema, Outputs, PortDef, PortType,
    Project, Value, evaluate_node,
};

struct ReportingNode(NodeSchema);
impl NodeKind for ReportingNode {
    fn schema(&self) -> &NodeSchema {
        &self.0
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        for fraction in [f32::NAN, -1.0, 0.25, 0.5, 1.5] {
            ctx.report_progress(fraction);
        }
        Ok(Outputs::from([(
            "out".into(),
            Value::Heightfield(Arc::new(Grid::filled(ctx.spec, 0.0))),
        )]))
    }
}
fn setup() -> (Project, NodeRegistry, String) {
    let mut registry = NodeRegistry::new();
    registry.register(ReportingNode(NodeSchema {
        type_id: "test.reporting".into(),
        type_version: 1,
        label: "Reporting".into(),
        category: "Test".into(),
        description: String::new(),
        inputs: vec![PortDef::new("in", "In", PortType::Heightfield).optional()],
        outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
        params: vec![],
        gpu: false,
    }));
    let mut project = Project::default();
    let a = project
        .graph
        .add_node(&registry, "test.reporting", [0.0, 0.0])
        .unwrap();
    let b = project
        .graph
        .add_node(&registry, "test.reporting", [0.0, 0.0])
        .unwrap();
    project.graph.connect(&registry, &a, "out", &b, "in").unwrap();
    (project, registry, b)
}
#[test]
fn node_progress_is_mapped_to_graph_progress() {
    let (p, registry, target) = setup();
    let samples = Mutex::new(Vec::new());
    let progress = |p| samples.lock().unwrap().push(p);
    evaluate_node(
        &p.graph,
        &registry,
        &p.world,
        GridSpec::full_world(&p.world, 2).unwrap(),
        &target,
        &EvalOptions {
            cancel: None,
            progress: Some(&progress),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        *samples.lock().unwrap(),
        vec![0.0, 0.125, 0.25, 0.5, 0.5, 0.5, 0.625, 0.75, 1.0, 1.0]
    );
}
#[test]
fn cancellation_during_final_node_never_returns_success() {
    let (p, registry, target) = setup();
    let cancel = AtomicBool::new(false);
    let progress = |p| {
        if p >= 0.625 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let result = evaluate_node(
        &p.graph,
        &registry,
        &p.world,
        GridSpec::full_world(&p.world, 2).unwrap(),
        &target,
        &EvalOptions {
            cancel: Some(&cancel),
            progress: Some(&progress),
            ..Default::default()
        },
    );
    assert!(matches!(result, Err(CoreError::Cancelled)));
}

#[test]
fn cancelled_results_are_never_cached() {
    let (p, registry, target) = setup();
    let cache = terrain_core::EvalCache::default();
    let spec = GridSpec::full_world(&p.world, 2).unwrap();
    let cancel = AtomicBool::new(false);
    let progress = |p| {
        if p >= 0.625 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let result = evaluate_node(
        &p.graph,
        &registry,
        &p.world,
        spec,
        &target,
        &EvalOptions {
            cancel: Some(&cancel),
            progress: Some(&progress),
            cache: Some(&cache),
            ..Default::default()
        },
    );
    assert!(matches!(result, Err(CoreError::Cancelled)));
    let cached_before = cache.stats().entries;
    // The same request again completes and computes the cancelled node.
    evaluate_node(
        &p.graph,
        &registry,
        &p.world,
        spec,
        &target,
        &EvalOptions {
            cache: Some(&cache),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(cache.stats().entries, cached_before + 1, "the cancelled node was cached");
}
