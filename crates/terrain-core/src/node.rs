use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::grid::{Grid, GridSpec};
use crate::params::{ParamDef, ParamValue};
use crate::world::World;

/// The type of data flowing through a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// Heights in metres.
    Heightfield,
    /// Values in 0..1 (masks, densities, weights).
    Mask,
}

impl PortType {
    /// Can data of type `self` feed an input of type `input`?
    /// Heightfield and Mask convert into each other automatically.
    pub fn can_feed(self, input: PortType) -> bool {
        matches!(
            (self, input),
            (
                PortType::Heightfield | PortType::Mask,
                PortType::Heightfield | PortType::Mask
            )
        )
    }
}

/// Data produced by a node output.
#[derive(Clone, Debug)]
pub enum Value {
    Heightfield(Arc<Grid>),
    Mask(Arc<Grid>),
}

impl Value {
    pub fn port_type(&self) -> PortType {
        match self {
            Value::Heightfield(_) => PortType::Heightfield,
            Value::Mask(_) => PortType::Mask,
        }
    }

    pub fn grid(&self) -> &Arc<Grid> {
        match self {
            Value::Heightfield(g) | Value::Mask(g) => g,
        }
    }

    /// Convert to another port type using the world height range.
    pub fn convert(&self, to: PortType, world: &World) -> Value {
        match (self, to) {
            (Value::Heightfield(g), PortType::Heightfield) => Value::Heightfield(g.clone()),
            (Value::Mask(g), PortType::Mask) => Value::Mask(g.clone()),
            (Value::Heightfield(g), PortType::Mask) => Value::Mask(Arc::new(g.map(|h| world.normalise(h)))),
            (Value::Mask(g), PortType::Heightfield) => {
                Value::Heightfield(Arc::new(g.map(|m| world.denormalise(m))))
            }
        }
    }
}

/// Declaration of one input or output port.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: PortType,
    /// Inputs only: may be left unconnected.
    pub optional: bool,
}

impl PortDef {
    pub fn new(key: &str, label: &str, ty: PortType) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            ty,
            optional: false,
        }
    }
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

/// Everything the editor needs to know about a node type.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeSchema {
    /// Stable identifier stored in project files, e.g. `noise.perlin`. Never rename.
    pub type_id: String,
    /// Bumped when parameters change meaning; see [`NodeKind::migrate`].
    pub type_version: u32,
    pub label: String,
    pub category: String,
    pub description: String,
    pub inputs: Vec<PortDef>,
    pub outputs: Vec<PortDef>,
    pub params: Vec<ParamDef>,
    /// Whether a GPU kernel exists (v0.4+). Always false in v0.1.
    pub gpu: bool,
}

impl NodeSchema {
    pub fn input(&self, key: &str) -> Option<&PortDef> {
        self.inputs.iter().find(|p| p.key == key)
    }
    pub fn output(&self, key: &str) -> Option<&PortDef> {
        self.outputs.iter().find(|p| p.key == key)
    }
    pub fn param(&self, key: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.key == key)
    }
}

/// Results of one node evaluation, by output port key.
pub type Outputs = BTreeMap<String, Value>;

/// A node type. Implement this (in `terrain-nodes`) to add a node to the app.
pub trait NodeKind: Send + Sync + 'static {
    fn schema(&self) -> &NodeSchema;

    /// Compute every output. Must be deterministic: same context in, same bits out.
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs>;

    /// Upgrade parameters saved by an older `type_version`. Default: no change.
    fn migrate(&self, _from_version: u32, _params: &mut BTreeMap<String, ParamValue>) -> Result<()> {
        Ok(())
    }
}

/// What a node sees while it evaluates.
pub struct EvalContext<'a> {
    pub world: &'a World,
    /// The grid (resolution + world region) being computed.
    pub spec: GridSpec,
    /// Node seed, derived from project seed, node id and the node's `seed` param.
    pub seed: u64,
    pub node_id: &'a str,
    pub(crate) schema: &'a NodeSchema,
    pub(crate) params: &'a BTreeMap<String, ParamValue>,
    pub(crate) inputs: BTreeMap<String, Value>,
    pub(crate) cancel: Option<&'a AtomicBool>,
}

impl<'a> EvalContext<'a> {
    /// Build a context directly (used by tests and tools; the evaluator builds its own).
    pub fn new(
        world: &'a World,
        spec: GridSpec,
        seed: u64,
        node_id: &'a str,
        schema: &'a NodeSchema,
        params: &'a BTreeMap<String, ParamValue>,
        inputs: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            world,
            spec,
            seed,
            node_id,
            schema,
            params,
            inputs,
            cancel: None,
        }
    }

    fn param(&self, key: &str) -> ParamValue {
        let def = self
            .schema
            .param(key)
            .unwrap_or_else(|| panic!("node '{}' has no parameter '{key}'", self.schema.type_id));
        self.params
            .get(key)
            .and_then(|v| def.validate(v).ok())
            .unwrap_or_else(|| def.default.clone())
    }

    pub fn f64(&self, key: &str) -> f64 {
        self.param(key).as_f64().unwrap_or(0.0)
    }
    pub fn f32(&self, key: &str) -> f32 {
        self.f64(key) as f32
    }
    pub fn i64(&self, key: &str) -> i64 {
        self.param(key).as_i64().unwrap_or(0)
    }
    pub fn bool(&self, key: &str) -> bool {
        self.param(key).as_bool().unwrap_or(false)
    }
    pub fn choice(&self, key: &str) -> String {
        self.param(key).as_str().unwrap_or_default().to_string()
    }

    /// An input, already converted to the port's declared type. `None` if unconnected.
    pub fn input(&self, key: &str) -> Option<&Value> {
        self.inputs.get(key)
    }

    /// An input grid; errors if a required input is missing.
    pub fn input_grid(&self, key: &str) -> Result<&Arc<Grid>> {
        self.inputs
            .get(key)
            .map(|v| v.grid())
            .ok_or_else(|| CoreError::MissingInput {
                node: self.node_id.into(),
                port: key.into(),
            })
    }

    /// True once the user has cancelled; long nodes should check this and stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// All node types known to the app, keyed by `type_id`.
#[derive(Default, Clone)]
pub struct NodeRegistry {
    kinds: BTreeMap<String, Arc<dyn NodeKind>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, kind: impl NodeKind) {
        let id = kind.schema().type_id.clone();
        assert!(!self.kinds.contains_key(&id), "node type '{id}' registered twice");
        self.kinds.insert(id, Arc::new(kind));
    }

    pub fn get(&self, type_id: &str) -> Option<&Arc<dyn NodeKind>> {
        self.kinds.get(type_id)
    }

    pub fn schema(&self, type_id: &str) -> Option<&NodeSchema> {
        self.kinds.get(type_id).map(|k| k.schema())
    }

    /// All schemas, sorted by type id.
    pub fn schemas(&self) -> impl Iterator<Item = &NodeSchema> {
        self.kinds.values().map(|k| k.schema())
    }
}
