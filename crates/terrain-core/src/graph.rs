use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::node::{NodeRegistry, PortDef};
use crate::params::ParamValue;

/// Stable node identifier, e.g. `n_0003`. Used for seeding and never reused.
pub type NodeId = String;

/// One node placed in the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeInstance {
    pub id: NodeId,
    pub type_id: String,
    pub type_version: u32,
    /// Position in the graph editor (editor units).
    pub pos: [f32; 2],
    /// Only parameters that differ from the default need to be present.
    pub params: BTreeMap<String, ParamValue>,
    /// Drivable parameters shown as input ports (see `ParamDef::drivable`).
    pub exposed: BTreeSet<String>,
}

/// A connection from an output port to an input port.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Link {
    pub from: (NodeId, String),
    pub to: (NodeId, String),
}

/// The node graph. Always acyclic; every input has at most one link.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub(crate) nodes: BTreeMap<NodeId, NodeInstance>,
    pub(crate) links: Vec<Link>,
    pub(crate) next_id: u64,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Default::default()
        }
    }

    pub fn nodes(&self) -> impl Iterator<Item = &NodeInstance> {
        self.nodes.values()
    }

    pub fn node(&self, id: &str) -> Option<&NodeInstance> {
        self.nodes.get(id)
    }

    pub fn links(&self) -> &[Link] {
        &self.links
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    fn node_mut(&mut self, id: &str) -> Result<&mut NodeInstance> {
        self.nodes
            .get_mut(id)
            .ok_or_else(|| CoreError::NodeNotFound(id.into()))
    }

    /// Add a node of a registered type. Returns its new id.
    pub fn add_node(&mut self, registry: &NodeRegistry, type_id: &str, pos: [f32; 2]) -> Result<NodeId> {
        let schema = registry
            .schema(type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(type_id.into()))?;
        let id = self.allocate_id();
        self.nodes.insert(
            id.clone(),
            NodeInstance {
                id: id.clone(),
                type_id: type_id.into(),
                type_version: schema.type_version,
                pos,
                params: BTreeMap::new(),
                exposed: BTreeSet::new(),
            },
        );
        Ok(id)
    }

    /// Input ports of a node: its schema's inputs plus exposed parameter ports.
    pub fn input_ports(&self, registry: &NodeRegistry, id: &str) -> Result<Vec<PortDef>> {
        let n = self
            .nodes
            .get(id)
            .ok_or_else(|| CoreError::NodeNotFound(id.into()))?;
        let schema = registry
            .schema(&n.type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(n.type_id.clone()))?;
        Ok(schema.input_ports(&n.exposed))
    }

    /// Show or hide a drivable parameter as an input port. Hiding it removes
    /// any link into the port.
    pub fn set_exposed(&mut self, registry: &NodeRegistry, id: &str, key: &str, exposed: bool) -> Result<()> {
        let type_id = self.node_mut(id)?.type_id.clone();
        let schema = registry
            .schema(&type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(type_id.clone()))?;
        if !schema.param(key).is_some_and(|d| d.drivable) {
            return Err(CoreError::InvalidParam {
                param: key.into(),
                reason: "this parameter can't be driven by a mask".into(),
            });
        }
        let node = self.node_mut(id)?;
        if exposed {
            node.exposed.insert(key.into());
        } else {
            node.exposed.remove(key);
            self.disconnect(id, &crate::node::param_port_key(key));
        }
        Ok(())
    }

    /// Insert a node exactly as given (used when loading projects).
    pub(crate) fn insert_raw(&mut self, node: NodeInstance) {
        if let Some(n) = node
            .id
            .strip_prefix("n_")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        {
            self.next_id = self.next_id.max(n + 1);
        }
        self.nodes.insert(node.id.clone(), node);
    }

    fn allocate_id(&mut self) -> NodeId {
        loop {
            let id = format!("n_{:04x}", self.next_id);
            self.next_id += 1;
            if !self.nodes.contains_key(&id) {
                return id;
            }
        }
    }

    /// Remove a node and every link touching it.
    pub fn remove_node(&mut self, id: &str) -> Result<()> {
        self.nodes
            .remove(id)
            .ok_or_else(|| CoreError::NodeNotFound(id.into()))?;
        self.links.retain(|l| l.from.0 != id && l.to.0 != id);
        Ok(())
    }

    pub fn set_position(&mut self, id: &str, pos: [f32; 2]) -> Result<()> {
        self.node_mut(id)?.pos = pos;
        Ok(())
    }

    /// Set a parameter, validated (and clamped) against the node's schema.
    /// Returns the value actually stored.
    pub fn set_param(
        &mut self,
        registry: &NodeRegistry,
        id: &str,
        key: &str,
        value: ParamValue,
    ) -> Result<ParamValue> {
        let type_id = self.node_mut(id)?.type_id.clone();
        let schema = registry
            .schema(&type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(type_id.clone()))?;
        let def = schema.param(key).ok_or_else(|| CoreError::InvalidParam {
            param: key.into(),
            reason: format!("'{type_id}' has no such parameter"),
        })?;
        let v = def.validate(&value)?;
        self.node_mut(id)?.params.insert(key.into(), v.clone());
        Ok(v)
    }

    /// Connect `from.from_port` to `to.to_port`, replacing any existing link into
    /// that input. Refuses type mismatches and cycles.
    pub fn connect(
        &mut self,
        registry: &NodeRegistry,
        from: &str,
        from_port: &str,
        to: &str,
        to_port: &str,
    ) -> Result<()> {
        let describe = || (format!("{from}.{from_port}"), format!("{to}.{to_port}"));
        let from_node = self
            .nodes
            .get(from)
            .ok_or_else(|| CoreError::NodeNotFound(from.into()))?;
        let out_ty = registry
            .schema(&from_node.type_id)
            .ok_or_else(|| CoreError::UnknownNodeType(from_node.type_id.clone()))?
            .output(from_port)
            .ok_or_else(|| CoreError::PortNotFound {
                node: from.into(),
                port: from_port.into(),
            })?
            .ty;
        let in_ty = self
            .input_ports(registry, to)?
            .into_iter()
            .find(|p| p.key == to_port)
            .ok_or_else(|| CoreError::PortNotFound {
                node: to.into(),
                port: to_port.into(),
            })?
            .ty;
        if !out_ty.can_feed(in_ty) {
            let (f, t) = describe();
            return Err(CoreError::InvalidLink {
                from: f,
                to: t,
                reason: format!("{out_ty:?} cannot feed {in_ty:?}"),
            });
        }
        if from == to || self.upstream(from).contains(to) {
            return Err(CoreError::Cycle);
        }
        self.links.retain(|l| !(l.to.0 == to && l.to.1 == to_port));
        self.links.push(Link {
            from: (from.into(), from_port.into()),
            to: (to.into(), to_port.into()),
        });
        self.links.sort();
        Ok(())
    }

    /// Remove the link into `to.to_port`, if any.
    pub fn disconnect(&mut self, to: &str, to_port: &str) {
        self.links.retain(|l| !(l.to.0 == to && l.to.1 == to_port));
    }

    /// The link feeding an input, if any.
    pub fn link_into(&self, to: &str, to_port: &str) -> Option<&Link> {
        self.links.iter().find(|l| l.to.0 == to && l.to.1 == to_port)
    }

    /// Every node that `id` depends on (not including `id`).
    pub fn upstream(&self, id: &str) -> BTreeSet<NodeId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![id.to_string()];
        while let Some(n) = stack.pop() {
            for l in self.links.iter().filter(|l| l.to.0 == n) {
                if seen.insert(l.from.0.clone()) {
                    stack.push(l.from.0.clone());
                }
            }
        }
        seen
    }

    /// Nodes needed to compute `target`, in dependency order (target last).
    pub fn evaluation_order(&self, target: &str) -> Result<Vec<NodeId>> {
        if !self.nodes.contains_key(target) {
            return Err(CoreError::NodeNotFound(target.into()));
        }
        let mut order = Vec::new();
        let mut state: BTreeMap<NodeId, bool> = BTreeMap::new(); // false = visiting, true = done
        self.visit(target, &mut state, &mut order)?;
        Ok(order)
    }

    fn visit(&self, id: &str, state: &mut BTreeMap<NodeId, bool>, order: &mut Vec<NodeId>) -> Result<()> {
        match state.get(id) {
            Some(true) => return Ok(()),
            Some(false) => return Err(CoreError::Cycle),
            None => {}
        }
        state.insert(id.into(), false);
        // Deterministic order: links are kept sorted.
        let deps: Vec<NodeId> = self
            .links
            .iter()
            .filter(|l| l.to.0 == id)
            .map(|l| l.from.0.clone())
            .collect();
        for d in deps {
            self.visit(&d, state, order)?;
        }
        state.insert(id.into(), true);
        order.push(id.into());
        Ok(())
    }
}
