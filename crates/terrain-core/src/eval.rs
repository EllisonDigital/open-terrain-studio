use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cache::{CacheKey, EvalCache, key_of};
use crate::error::{CoreError, Result};
use crate::graph::{Graph, NodeInstance};
use crate::grid::GridSpec;
use crate::node::{EvalContext, NodeKind, NodeRegistry, Outputs};
use crate::seed;
use crate::world::World;

/// Optional controls for an evaluation.
#[derive(Default)]
pub struct EvalOptions<'a> {
    /// Set to true from another thread to stop early.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with overall progress 0..1 after each node finishes.
    pub progress: Option<&'a (dyn Fn(f32) + Sync)>,
    /// Reuse and store node results here. Without a cache everything is computed.
    pub cache: Option<&'a EvalCache>,
    /// Folder that relative file paths (e.g. imported heightmaps) are resolved
    /// against: the project file's folder.
    pub base_dir: Option<&'a Path>,
}

/// Evaluate `target` (and everything upstream of it) over `spec`.
///
/// Nodes run one after another in dependency order; each parallelises
/// internally over rows. With a cache, a node whose inputs, parameters and
/// grid are unchanged is not recomputed.
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
    let mut keys: BTreeMap<String, CacheKey> = BTreeMap::new();
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
        let ports = schema.input_ports(&node.exposed);
        let seed_param = node.params.get("seed").and_then(|v| v.as_i64()).unwrap_or(0);
        let node_seed = seed::node_seed(world.seed, id, seed_param);

        // Gather inputs, converting to each port's declared type.
        let mut inputs = BTreeMap::new();
        let mut input_keys = Vec::new();
        for port in &ports {
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
                    input_keys.push((port.key.as_str(), keys[&link.from.0], link.from.1.as_str()));
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

        let key = cache_key(
            kind.as_ref(),
            node,
            node_seed,
            spec,
            world,
            &input_keys,
            opts.base_dir,
        );
        keys.insert(id.clone(), key);

        let cached = opts.cache.and_then(|c| c.get(key));
        let outputs = match cached {
            Some(outputs) => outputs,
            None => {
                let mut ctx = EvalContext::new(world, spec, node_seed, id, schema, &node.params, inputs);
                ctx.cancel = opts.cancel;
                ctx.base_dir = opts.base_dir;
                let outputs = kind.evaluate(&ctx).map_err(|e| match e {
                    CoreError::Cancelled | CoreError::MissingInput { .. } => e,
                    other => CoreError::NodeFailed {
                        node: id.clone(),
                        message: other.to_string(),
                    },
                })?;
                if let Some(c) = opts.cache {
                    c.insert(key, &outputs);
                }
                outputs
            }
        };
        results.insert(id.clone(), outputs);

        if let Some(p) = opts.progress {
            p((n + 1) as f32 / total);
        }
    }

    results
        .remove(target)
        .ok_or_else(|| CoreError::NodeNotFound(target.into()))
}

/// The cache key of one node result: a hash of everything it depends on.
fn cache_key(
    kind: &dyn NodeKind,
    node: &NodeInstance,
    node_seed: u64,
    spec: GridSpec,
    world: &World,
    inputs: &[(&str, CacheKey, &str)],
    base_dir: Option<&Path>,
) -> CacheKey {
    let schema = kind.schema();
    let mut text = String::with_capacity(512);
    let _ = write!(
        text,
        "{}@{}|seed={node_seed}|grid={spec:?}|world={world:?}|",
        schema.type_id, schema.type_version
    );
    // Effective values (defaults filled in), in schema order.
    for def in &schema.params {
        let v = node
            .params
            .get(&def.key)
            .and_then(|v| def.validate(v).ok())
            .unwrap_or_else(|| def.default.clone());
        let _ = write!(
            text,
            "{}={};",
            def.key,
            serde_json::to_string(&v).unwrap_or_default()
        );
    }
    for (port, key, out) in inputs {
        let _ = write!(text, "|in:{port}={key:032x}.{out}");
    }
    let salt = kind.cache_salt(&node.params, base_dir);
    if !salt.is_empty() {
        let _ = write!(text, "|salt={salt}");
    }
    key_of(&text)
}
