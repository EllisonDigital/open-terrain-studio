use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{CoreError, Result};
use crate::graph::Graph;
use crate::grid::GridSpec;
use crate::node::{EvalContext, NodeRegistry, Outputs};
use crate::seed;
use crate::world::World;

/// Optional controls for an evaluation.
#[derive(Default)]
pub struct EvalOptions<'a> {
    /// Set to true from another thread to stop early.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with overall progress 0..1 after each node finishes.
    pub progress: Option<&'a (dyn Fn(f32) + Sync)>,
}

/// Evaluate `target` (and everything upstream of it) over `spec`.
///
/// v0.1 evaluates nodes one after another in dependency order; each node
/// parallelises internally over rows. Persistent caching arrives in v0.2.
pub fn evaluate_node(
    graph: &Graph,
    registry: &NodeRegistry,
    world: &World,
    spec: GridSpec,
    target: &str,
    opts: &EvalOptions,
) -> Result<Outputs> {
    let order = graph.evaluation_order(target)?;
    let mut results: BTreeMap<String, Outputs> = BTreeMap::new();
    let total = order.len() as f32;

    for (n, id) in order.iter().enumerate() {
        if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        let node = graph
            .node(id)
            .ok_or_else(|| CoreError::NodeNotFound(id.clone()))?;
        let kind = registry
            .get(&node.type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(node.type_id.clone()))?;
        let schema = kind.schema();

        // Gather inputs, converting to each port's declared type.
        let mut inputs = BTreeMap::new();
        for port in &schema.inputs {
            match graph.link_into(id, &port.key) {
                Some(link) => {
                    let value = results
                        .get(&link.from.0)
                        .and_then(|o| o.get(&link.from.1))
                        .ok_or_else(|| CoreError::PortNotFound {
                            node: link.from.0.clone(),
                            port: link.from.1.clone(),
                        })?;
                    inputs.insert(port.key.clone(), value.convert(port.ty, world));
                }
                None if port.optional => {}
                None => {
                    return Err(CoreError::MissingInput {
                        node: id.clone(),
                        port: port.label.clone(),
                    });
                }
            }
        }

        let seed_param = node.params.get("seed").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut ctx = EvalContext::new(
            world,
            spec,
            seed::node_seed(world.seed, id, seed_param),
            id,
            schema,
            &node.params,
            inputs,
        );
        ctx.cancel = opts.cancel;

        let outputs = kind.evaluate(&ctx).map_err(|e| match e {
            CoreError::Cancelled | CoreError::MissingInput { .. } => e,
            other => CoreError::NodeFailed {
                node: id.clone(),
                message: other.to_string(),
            },
        })?;
        results.insert(id.clone(), outputs);

        if let Some(p) = opts.progress {
            p((n + 1) as f32 / total);
        }
    }

    results
        .remove(target)
        .ok_or_else(|| CoreError::NodeNotFound(target.into()))
}
