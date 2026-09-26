use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cache::{CacheKey, EvalCache, key_of};
use crate::error::{CoreError, Result};
use crate::gpu::Gpu;
use crate::graph::{Graph, NodeInstance};
use crate::grid::GridSpec;
use crate::node::{EvalContext, NodeKind, NodeRegistry, Outputs, PortDef, Value};
use crate::seed;
use crate::world::World;

/// Optional controls for an evaluation.
#[derive(Default)]
pub struct EvalOptions<'a> {
    /// Set to true from another thread to stop early.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with overall progress 0..1, including updates within long nodes.
    pub progress: Option<&'a (dyn Fn(f32) + Sync)>,
    /// Reuse and store node results here. Without a cache everything is computed.
    pub cache: Option<&'a EvalCache>,
    /// Folder that relative file paths (e.g. imported heightmaps) are resolved
    /// against: the project file's folder.
    pub base_dir: Option<&'a Path>,
    /// Run nodes that have a GPU kernel on this device. Their results match
    /// the CPU within tolerance rather than bit for bit, so they are cached
    /// separately. `None` = everything on the CPU (bit-exact).
    pub gpu: Option<&'a Gpu>,
}

/// Evaluate `target` (and everything upstream of it) over `spec`.
///
/// Nodes run one after another in dependency order; each parallelises
/// internally over rows. With a cache, a node whose inputs, parameters and
/// grid are unchanged is not recomputed. A result is dropped as soon as
/// nothing left to run needs it.
pub fn evaluate_node(
    graph: &Graph,
    registry: &NodeRegistry,
    world: &World,
    spec: GridSpec,
    target: &str,
    opts: &EvalOptions,
) -> Result<Outputs> {
    let order = graph.evaluation_order(target)?;
    let mut uses = consumer_counts(graph, &order);
    let mut results: BTreeMap<String, (Outputs, CacheKey)> = BTreeMap::new();
    let total = order.len() as f32;

    for (n, id) in order.iter().enumerate() {
        let step = NodeStep::new(graph, registry, world, id)?;
        let (inputs, input_keys) =
            step.gather(graph, world, spec, |from| results.get(from).map(|(o, k)| (o, *k)))?;
        let node_progress = |fraction: f32| {
            if let Some(p) = opts.progress {
                p((n as f32 + fraction) / total);
            }
        };
        let result = step.run(world, spec, inputs, &input_keys, opts, &node_progress, None)?;
        release_inputs(graph, id, &mut uses, &mut results);
        results.insert(id.clone(), result);

        if let Some(p) = opts.progress {
            p((n + 1) as f32 / total);
        }
    }

    results
        .remove(target)
        .map(|(o, _)| o)
        .ok_or_else(|| CoreError::NodeNotFound(target.into()))
}

/// How many links out of each node in `order` lead to another node in it.
pub(crate) fn consumer_counts(graph: &Graph, order: &[String]) -> BTreeMap<String, usize> {
    let mut uses: BTreeMap<String, usize> = order.iter().map(|id| (id.clone(), 0)).collect();
    for link in graph.links() {
        if uses.contains_key(&link.to.0)
            && let Some(u) = uses.get_mut(&link.from.0)
        {
            *u += 1;
        }
    }
    uses
}

/// After `id` has run: drop the results of its inputs that nothing else
/// still needs. Nodes with no consumers left (the targets) are kept.
pub(crate) fn release_inputs<T>(
    graph: &Graph,
    id: &str,
    uses: &mut BTreeMap<String, usize>,
    results: &mut BTreeMap<String, T>,
) {
    for link in graph.links().iter().filter(|l| l.to.0 == id) {
        if let Some(u) = uses.get_mut(&link.from.0) {
            *u = u.saturating_sub(1);
            if *u == 0 {
                results.remove(&link.from.0);
            }
        }
    }
}

/// `(input port, producer's cache key, producer's output port)` per
/// connected input, for a cache key.
pub(crate) type InputKeys = Vec<(String, CacheKey, String)>;

/// Everything needed to run one node: its instance, kind, seed and ports.
pub(crate) struct NodeStep<'g> {
    pub id: &'g str,
    pub node: &'g NodeInstance,
    pub kind: &'g Arc<dyn NodeKind>,
    pub seed: u64,
    pub ports: Vec<PortDef>,
}

/// A custom computation for [`NodeStep::run`], mixed into the cache key as
/// `tag`.
pub(crate) struct Compute<'c> {
    pub tag: String,
    #[allow(clippy::type_complexity)]
    pub run: &'c dyn Fn(&EvalContext) -> Result<Outputs>,
}

impl<'g> NodeStep<'g> {
    pub fn new(graph: &'g Graph, registry: &'g NodeRegistry, world: &World, id: &'g str) -> Result<Self> {
        let node = graph.node(id).ok_or_else(|| CoreError::NodeNotFound(id.into()))?;
        let kind = registry
            .get(&node.type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(node.type_id.clone()))?;
        let seed_param = node.params.get("seed").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(Self {
            id,
            node,
            kind,
            seed: seed::node_seed(world.seed, id, seed_param),
            ports: kind.schema().input_ports(&node.exposed),
        })
    }

    /// A context with this node's parameters over `spec` and no inputs
    /// (for [`NodeKind::reach`] and [`NodeKind::world_spec`]).
    pub fn bare_context<'w>(&'w self, world: &'w World, spec: GridSpec) -> EvalContext<'w> {
        EvalContext::new(
            world,
            spec,
            self.seed,
            self.id,
            self.kind.schema(),
            &self.node.params,
            BTreeMap::new(),
        )
    }

    /// Collect this node's inputs over `spec`, converted to each port's type
    /// and cropped to `spec`, from `lookup(producer) = (outputs, cache key)`.
    /// Returns the inputs and `(port, producer key, producer port)` for the
    /// cache key.
    pub fn gather<'r>(
        &self,
        graph: &Graph,
        world: &World,
        spec: GridSpec,
        lookup: impl Fn(&str) -> Option<(&'r Outputs, CacheKey)>,
    ) -> Result<(BTreeMap<String, Value>, InputKeys)> {
        let mut inputs = BTreeMap::new();
        let mut keys = Vec::new();
        for port in &self.ports {
            match graph.link_into(self.id, &port.key) {
                Some(link) => {
                    let missing = || CoreError::PortNotFound {
                        node: link.from.0.clone(),
                        port: link.from.1.clone(),
                    };
                    let (outputs, key) = lookup(&link.from.0).ok_or_else(missing)?;
                    let value = outputs.get(&link.from.1).ok_or_else(missing)?;
                    inputs.insert(port.key.clone(), value.crop(spec).convert(port.ty, world));
                    keys.push((port.key.clone(), key, link.from.1.clone()));
                }
                None if port.optional => {}
                None => {
                    return Err(CoreError::MissingInput {
                        node: self.id.into(),
                        port: port.label.clone(),
                    });
                }
            }
        }
        Ok((inputs, keys))
    }

    /// Run the node over `spec` (or reuse its cached result) and cache it.
    /// `custom` replaces the node's own evaluation (e.g. finishing a tile of
    /// a global node) and is always on the CPU.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        world: &World,
        spec: GridSpec,
        inputs: BTreeMap<String, Value>,
        input_keys: &InputKeys,
        opts: &EvalOptions,
        progress: &(dyn Fn(f32) + Sync),
        custom: Option<&Compute>,
    ) -> Result<(Outputs, CacheKey)> {
        if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        let schema = self.kind.schema();
        let gpu = opts.gpu.filter(|_| schema.gpu && custom.is_none());
        let key = cache_key(
            self.kind.as_ref(),
            self.node,
            self.seed,
            spec,
            world,
            input_keys,
            opts.base_dir,
            gpu.is_some(),
            custom.map_or("", |c| c.tag.as_str()),
        );
        if let Some(outputs) = opts.cache.and_then(|c| c.get(key)) {
            return Ok((outputs, key));
        }
        let mut ctx = EvalContext::new(world, spec, self.seed, self.id, schema, &self.node.params, inputs);
        ctx.cancel = opts.cancel;
        ctx.progress = Some(progress);
        ctx.base_dir = opts.base_dir;
        let on_gpu = match gpu {
            Some(gpu) => match self.kind.evaluate_gpu(&ctx, gpu) {
                Ok(outputs) => {
                    gpu.record_node();
                    Some(outputs)
                }
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(CoreError::GpuUnsupported) => None,
                // Anything else (a driver or kernel problem): use the CPU.
                Err(e) => {
                    gpu.record_fallback(&e);
                    None
                }
            },
            None => None,
        };
        let outputs = match (on_gpu, custom) {
            (Some(outputs), _) => Ok(outputs),
            (None, Some(c)) => (c.run)(&ctx),
            (None, None) => self.kind.evaluate(&ctx),
        };
        let outputs = outputs.map_err(|e| match e {
            CoreError::Cancelled | CoreError::MissingInput { .. } => e,
            other => CoreError::NodeFailed {
                node: self.id.into(),
                message: other.to_string(),
            },
        })?;
        // A node that stopped early may return partial results: never cache them.
        if ctx.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        if let Some(c) = opts.cache {
            c.insert(key, &outputs);
        }
        Ok((outputs, key))
    }
}

/// The cache key of one node result: a hash of everything it depends on.
#[allow(clippy::too_many_arguments)]
fn cache_key(
    kind: &dyn NodeKind,
    node: &NodeInstance,
    node_seed: u64,
    spec: GridSpec,
    world: &World,
    inputs: &[(String, CacheKey, String)],
    base_dir: Option<&Path>,
    gpu: bool,
    tag: &str,
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
    if gpu {
        text.push_str("|gpu");
    }
    if !tag.is_empty() {
        let _ = write!(text, "|{tag}");
    }
    key_of(&text)
}
