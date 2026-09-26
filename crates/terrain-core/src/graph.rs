use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::node::{NodeRegistry, PortDef};
use crate::params::ParamValue;

/// Stable node identifier, e.g. `n_0003`. Used for seeding and never reused.
pub type NodeId = String;

/// The graph editor tab a node belongs to (ARCHITECTURE.md §5). Tabs only
/// organise the editor: links may cross them (the Vegetation and Colour tabs
/// read other tabs' outputs through portal nodes) and evaluation ignores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Terrain,
    Vegetation,
    Colour,
}

impl Tab {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "terrain" => Some(Self::Terrain),
            "vegetation" => Some(Self::Vegetation),
            "colour" => Some(Self::Colour),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Terrain => "terrain",
            Self::Vegetation => "vegetation",
            Self::Colour => "colour",
        }
    }
    /// Name shown in the editor, e.g. "Vegetation".
    pub fn label(self) -> &'static str {
        match self {
            Self::Terrain => "Terrain",
            Self::Vegetation => "Vegetation",
            Self::Colour => "Colour",
        }
    }
}

/// The portal node type that carries data of type `ty` between tabs, if any
/// (heights and masks only).
pub fn portal_type(ty: crate::node::PortType) -> Option<&'static str> {
    match ty {
        crate::node::PortType::Heightfield => Some("portal.height"),
        crate::node::PortType::Mask => Some("portal.mask"),
        _ => None,
    }
}

/// Vertical distance between portals stacked by [`Graph::move_vegetation_to_tab`].
const PORTAL_STEP: f32 = 140.0;

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
    /// Editor tab the node is shown in.
    pub tab: Tab,
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
                tab: Tab::Terrain,
            },
        );
        Ok(id)
    }

    /// Move a node to another editor tab.
    pub fn set_tab(&mut self, id: &str, tab: Tab) -> Result<()> {
        self.node_mut(id)?.tab = tab;
        Ok(())
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

    /// Bring `from.from_port` into `tab` through a new portal at `pos`, and
    /// return the portal's id. Only heights and masks travel through portals.
    pub fn add_portal(
        &mut self,
        registry: &NodeRegistry,
        from: &str,
        from_port: &str,
        tab: Tab,
        pos: [f32; 2],
    ) -> Result<NodeId> {
        let from_node = self
            .nodes
            .get(from)
            .ok_or_else(|| CoreError::NodeNotFound(from.into()))?;
        let ty = registry
            .schema(&from_node.type_id)
            .and_then(|s| s.output(from_port))
            .ok_or_else(|| CoreError::PortNotFound {
                node: from.into(),
                port: from_port.into(),
            })?
            .ty;
        let portal = portal_type(ty).ok_or_else(|| CoreError::InvalidLink {
            from: format!("{from}.{from_port}"),
            to: format!("the {} tab", tab.label()),
            reason: "only heights and masks travel through portals".into(),
        })?;
        let id = self.add_node(registry, portal, pos)?;
        self.set_tab(&id, tab)?;
        self.connect(registry, from, from_port, &id, "in")?;
        Ok(id)
    }

    /// Put every Vegetation node that sits in the Terrain tab (as in v0.7
    /// projects) in the Vegetation tab, and route every height or mask link
    /// that then crosses tabs through a portal in the receiving tab, so no link
    /// is hidden. Results are unchanged: a portal passes its input through.
    /// Returns how many nodes moved.
    pub fn move_vegetation_to_tab(&mut self, registry: &NodeRegistry) -> usize {
        let moved: Vec<NodeId> = self
            .nodes
            .values()
            .filter(|n| {
                n.tab == Tab::Terrain
                    && registry
                        .schema(&n.type_id)
                        .is_some_and(|s| s.category == "Vegetation")
            })
            .map(|n| n.id.clone())
            .collect();
        if moved.is_empty() {
            return 0;
        }
        for id in &moved {
            if let Some(n) = self.nodes.get_mut(id) {
                n.tab = Tab::Vegetation;
            }
        }
        // Links that now cross tabs into something other than a portal.
        let crossing: Vec<Link> = self
            .links
            .iter()
            .filter(|l| {
                let (Some(a), Some(b)) = (self.nodes.get(&l.from.0), self.nodes.get(&l.to.0)) else {
                    return false;
                };
                a.tab != b.tab && !b.type_id.starts_with("portal.")
            })
            .cloned()
            .collect();
        // One portal per source output and receiving tab, stacked left of that tab's nodes.
        let mut portals: BTreeMap<(NodeId, String, Tab), NodeId> = BTreeMap::new();
        let mut stacked: BTreeMap<Tab, usize> = BTreeMap::new();
        for link in crossing {
            let tab = self.nodes[&link.to.0].tab;
            let key = (link.from.0.clone(), link.from.1.clone(), tab);
            let portal = match portals.get(&key) {
                Some(p) => p.clone(),
                None => {
                    let (left, top) = self
                        .nodes
                        .values()
                        .filter(|n| n.tab == tab)
                        .fold((f32::INFINITY, f32::INFINITY), |(x, y), n| {
                            (x.min(n.pos[0]), y.min(n.pos[1]))
                        });
                    let k = stacked.entry(tab).or_insert(0);
                    let pos = [left - 340.0, top + PORTAL_STEP * *k as f32];
                    let Ok(p) = self.add_portal(registry, &link.from.0, &link.from.1, tab, pos) else {
                        // Points and colour maps have no portal: leave the link as it is.
                        continue;
                    };
                    *k += 1;
                    portals.insert(key, p.clone());
                    p
                }
            };
            // Replacing the link into this input keeps the graph acyclic.
            let _ = self.connect(registry, &portal, "out", &link.to.0, &link.to.1);
        }
        moved.len()
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

    /// Nodes that put water on the terrain `id` produces, for drawing water in
    /// the 3D view: `id` and every node upstream with both a `height` and a
    /// `water_surface` Heightfield output (Rivers, Lakes, Sea), in id order.
    pub fn water_sources(&self, registry: &NodeRegistry, id: &str) -> Vec<NodeId> {
        use crate::node::PortType::Heightfield;
        self.upstream_with(
            registry,
            id,
            &[("height", Heightfield), ("water_surface", Heightfield)],
        )
    }

    /// Vegetation and debris nodes whose points to draw with `id`'s output in
    /// the 3D view: `id` and every node upstream with a `points` PointSet
    /// output (e.g. earlier populations feeding an Occupied input), in id order.
    pub fn vegetation_sources(&self, registry: &NodeRegistry, id: &str) -> Vec<NodeId> {
        self.upstream_with(registry, id, &[("points", crate::node::PortType::PointSet)])
    }

    /// Nodes that put snow on the terrain `id` produces, for drawing it in the
    /// 3D view: `id` and every node upstream with a `height` Heightfield and a
    /// `snow` Mask output (Snow), in id order.
    pub fn snow_sources(&self, registry: &NodeRegistry, id: &str) -> Vec<NodeId> {
        use crate::node::PortType::{Heightfield, Mask};
        self.upstream_with(registry, id, &[("height", Heightfield), ("snow", Mask)])
    }

    /// `id` and the nodes upstream of it that have every one of `outputs`.
    fn upstream_with(
        &self,
        registry: &NodeRegistry,
        id: &str,
        outputs: &[(&str, crate::node::PortType)],
    ) -> Vec<NodeId> {
        let mut ids = self.upstream(id);
        ids.insert(id.to_string());
        ids.into_iter()
            .filter(|n| {
                let Some(schema) = self.nodes.get(n).and_then(|n| registry.schema(&n.type_id)) else {
                    return false;
                };
                outputs
                    .iter()
                    .all(|(key, ty)| schema.output(key).is_some_and(|o| o.ty == *ty))
            })
            .collect()
    }

    /// The terrain a mask output of `id` belongs to, for draping the mask over
    /// it in the 3D view: the node's own Heightfield output if it has one (e.g.
    /// an erosion node's Height next to its Flow mask), otherwise the nearest
    /// Heightfield output upstream (breadth-first, inputs in port order).
    /// `None` if there isn't one.
    pub fn base_heightfield(&self, registry: &NodeRegistry, id: &str) -> Option<(NodeId, String)> {
        let own = self
            .nodes
            .get(id)
            .and_then(|n| registry.schema(&n.type_id))
            .and_then(|s| {
                s.outputs
                    .iter()
                    .find(|o| o.ty == crate::node::PortType::Heightfield)
            });
        if let Some(port) = own {
            return Some((id.to_string(), port.key.clone()));
        }
        let mut queue = std::collections::VecDeque::from([id.to_string()]);
        let mut seen = BTreeSet::from([id.to_string()]);
        while let Some(n) = queue.pop_front() {
            let Ok(ports) = self.input_ports(registry, &n) else {
                continue;
            };
            for port in ports {
                let Some(link) = self.link_into(&n, &port.key) else {
                    continue;
                };
                let from = &link.from;
                let ty = self
                    .nodes
                    .get(&from.0)
                    .and_then(|f| registry.schema(&f.type_id))
                    .and_then(|s| s.output(&from.1))
                    .map(|o| o.ty);
                if ty == Some(crate::node::PortType::Heightfield) {
                    return Some(from.clone());
                }
                if seen.insert(from.0.clone()) {
                    queue.push_back(from.0.clone());
                }
            }
        }
        None
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

    /// Nodes needed to evaluate all of `targets`, in dependency order.
    pub fn evaluation_order_of(&self, targets: &[&str]) -> Result<Vec<NodeId>> {
        let mut order = Vec::new();
        let mut state: BTreeMap<NodeId, bool> = BTreeMap::new();
        for target in targets {
            if !self.nodes.contains_key(*target) {
                return Err(CoreError::NodeNotFound((*target).into()));
            }
            self.visit(target, &mut state, &mut order)?;
        }
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
