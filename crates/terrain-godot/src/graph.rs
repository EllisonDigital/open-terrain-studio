use godot::prelude::*;
use terrain_core::preset::SpeciesPreset;
use terrain_core::{NodeSchema, PortType, Tab};

use crate::convert::{json_to_variant, param_to_variant, put, variant_to_param};
use crate::project::{Shared, lock};
use crate::registry;

/// The node graph of a `TerrainProject`. A live view: every edit goes straight
/// into the project owned by Rust (Godot never holds the source of truth), and
/// every edit is undoable through the project.
#[derive(GodotClass)]
#[class(base=RefCounted, no_init)]
pub struct TerrainGraph {
    shared: Shared,
    last_error: GString,
    base: Base<RefCounted>,
}

impl TerrainGraph {
    pub(crate) fn for_project(shared: Shared) -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            shared,
            last_error: GString::new(),
            base,
        })
    }

    fn fail(&mut self, message: String) {
        self.last_error = message.as_str().into();
    }

    fn schema_dict(schema: &NodeSchema) -> Variant {
        json_to_variant(&serde_json::to_value(schema).unwrap_or_default())
    }

    /// "Mountain" for a node id, for undo labels.
    fn label_of(&self, id: &str) -> String {
        lock(&self.shared)
            .project
            .graph
            .node(id)
            .and_then(|n| registry().schema(&n.type_id))
            .map(|s| s.label.clone())
            .unwrap_or_else(|| "node".into())
    }
}

#[godot_api]
impl TerrainGraph {
    /// Every available node type: type_id, label, category, description,
    /// inputs, outputs and params (key, label, kind, min, max, step, options,
    /// filters, default, unit, description, drivable).
    #[func]
    fn get_node_types(&self) -> VarArray {
        let mut arr = VarArray::new();
        for schema in registry().schemas() {
            arr.push(&Self::schema_dict(schema));
        }
        arr
    }

    /// Schema of one node type, or an empty dictionary if unknown.
    #[func]
    fn get_node_type(&self, type_id: GString) -> Variant {
        registry()
            .schema(&type_id.to_string())
            .map(Self::schema_dict)
            .unwrap_or_else(|| VarDictionary::new().to_variant())
    }

    /// All nodes: id, type, label, category, pos (Vector2), known (bool), gpu
    /// (has a GPU kernel),
    /// inputs (including exposed parameter ports, which have a "param" key),
    /// outputs, params (effective values, defaults filled in), exposed
    /// (PackedStringArray of parameter keys) and exported (PackedStringArray of
    /// output ports marked for export).
    #[func]
    fn get_nodes(&self) -> VarArray {
        let s = lock(&self.shared);
        let mut arr = VarArray::new();
        for node in s.project.graph.nodes() {
            let mut d = VarDictionary::new();
            put(&mut d, "id", GString::from(node.id.as_str()));
            put(&mut d, "type", GString::from(node.type_id.as_str()));
            put(&mut d, "pos", Vector2::new(node.pos[0], node.pos[1]));
            put(&mut d, "tab", node.tab.key());
            let mut params = VarDictionary::new();
            match registry().schema(&node.type_id) {
                Some(schema) => {
                    put(&mut d, "known", true);
                    put(&mut d, "label", GString::from(schema.label.as_str()));
                    put(&mut d, "category", GString::from(schema.category.as_str()));
                    put(&mut d, "gpu", schema.gpu);
                    let inputs = serde_json::to_value(schema.input_ports(&node.exposed)).unwrap_or_default();
                    let outputs = serde_json::to_value(&schema.outputs).unwrap_or_default();
                    put(&mut d, "inputs", json_to_variant(&inputs));
                    put(&mut d, "outputs", json_to_variant(&outputs));
                    for def in &schema.params {
                        let v = node
                            .params
                            .get(&def.key)
                            .and_then(|v| def.validate(v).ok())
                            .unwrap_or_else(|| def.default.clone());
                        params.set(
                            &GString::from(def.key.as_str()).to_variant(),
                            &param_to_variant(&v),
                        );
                    }
                }
                None => {
                    put(&mut d, "known", false);
                    put(
                        &mut d,
                        "label",
                        GString::from(format!("Unknown: {}", node.type_id).as_str()),
                    );
                    put(&mut d, "category", GString::from("Unknown"));
                    put(&mut d, "gpu", false);
                    put(&mut d, "inputs", VarArray::new());
                    put(&mut d, "outputs", VarArray::new());
                    for (k, v) in &node.params {
                        params.set(&GString::from(k.as_str()).to_variant(), &param_to_variant(v));
                    }
                }
            }
            put(&mut d, "params", params);
            put(
                &mut d,
                "exposed",
                node.exposed
                    .iter()
                    .map(|k| GString::from(k.as_str()))
                    .collect::<PackedStringArray>(),
            );
            let mut exported: Vec<&str> = s
                .project
                .exports
                .iter()
                .filter(|e| e.node == node.id)
                .map(|e| e.port.as_str())
                .collect();
            exported.dedup();
            put(
                &mut d,
                "exported",
                exported
                    .into_iter()
                    .map(GString::from)
                    .collect::<PackedStringArray>(),
            );
            arr.push(&d.to_variant());
        }
        arr
    }

    /// All links: from, from_port, to, to_port.
    #[func]
    fn get_links(&self) -> VarArray {
        let s = lock(&self.shared);
        let mut arr = VarArray::new();
        for l in s.project.graph.links() {
            let mut d = VarDictionary::new();
            put(&mut d, "from", GString::from(l.from.0.as_str()));
            put(&mut d, "from_port", GString::from(l.from.1.as_str()));
            put(&mut d, "to", GString::from(l.to.0.as_str()));
            put(&mut d, "to_port", GString::from(l.to.1.as_str()));
            arr.push(&d.to_variant());
        }
        arr
    }

    #[func]
    fn has_node(&self, id: GString) -> bool {
        lock(&self.shared).project.graph.node(&id.to_string()).is_some()
    }

    /// Add a node. Returns its id, or "" on error.
    #[func]
    fn add_node(&mut self, type_id: GString, pos: Vector2) -> GString {
        let type_id = type_id.to_string();
        let label = registry()
            .schema(&type_id)
            .map(|s| format!("Add {}", s.label))
            .unwrap_or_default();
        let result = lock(&self.shared).edit(&label, None, |p| {
            p.graph.add_node(registry(), &type_id, [pos.x, pos.y])
        });
        match result {
            Ok(id) => id.as_str().into(),
            Err(e) => {
                self.fail(e.to_string());
                GString::new()
            }
        }
    }

    /// Add a node to an editor tab ("terrain" or "colour"). Returns its id, or
    /// "" on error.
    #[func]
    fn add_node_in_tab(&mut self, type_id: GString, pos: Vector2, tab: GString) -> GString {
        let Some(tab) = Tab::parse(&tab.to_string()) else {
            self.fail(format!("unknown tab '{tab}'"));
            return GString::new();
        };
        let type_id = type_id.to_string();
        let label = registry()
            .schema(&type_id)
            .map(|s| format!("Add {}", s.label))
            .unwrap_or_default();
        let result = lock(&self.shared).edit(&label, None, |p| {
            let id = p.graph.add_node(registry(), &type_id, [pos.x, pos.y])?;
            p.graph.set_tab(&id, tab)?;
            Ok(id)
        });
        match result {
            Ok(id) => id.as_str().into(),
            Err(e) => {
                self.fail(e.to_string());
                GString::new()
            }
        }
    }

    /// Bring a Terrain-tab output into the Colour tab: adds a Height or Mask
    /// Portal there at `pos`, linked to `from`'s output `from_port`. Returns
    /// the portal's id, or "" on error (e.g. for a colour map).
    #[func]
    fn send_to_colour_tab(&mut self, from: GString, from_port: GString, pos: Vector2) -> GString {
        let (from, from_port) = (from.to_string(), from_port.to_string());
        let ty = lock(&self.shared)
            .project
            .graph
            .node(&from)
            .and_then(|n| registry().schema(&n.type_id))
            .and_then(|s| s.output(&from_port))
            .map(|o| o.ty);
        let portal = match ty {
            Some(PortType::Heightfield) => "portal.height",
            Some(PortType::Mask) => "portal.mask",
            Some(PortType::ColorMap | PortType::PointSet) => {
                self.fail(
                    "only heights and masks can be sent to the Colour tab (colour maps are made there; points aren't colours)"
                        .into(),
                );
                return GString::new();
            }
            None => {
                self.fail(format!("node {from} has no output '{from_port}'"));
                return GString::new();
            }
        };
        let result = lock(&self.shared).edit("Send to Colour tab", None, |p| {
            let id = p.graph.add_node(registry(), portal, [pos.x, pos.y])?;
            p.graph.set_tab(&id, Tab::Colour)?;
            p.graph.connect(registry(), &from, &from_port, &id, "in")?;
            Ok(id)
        });
        match result {
            Ok(id) => id.as_str().into(),
            Err(e) => {
                self.fail(e.to_string());
                GString::new()
            }
        }
    }

    /// Remove a node, its links and its export marks.
    #[func]
    fn remove_node(&mut self, id: GString) -> bool {
        let id = id.to_string();
        let label = format!("Delete {}", self.label_of(&id));
        let result = lock(&self.shared).edit(&label, None, |p| p.remove_node(&id));
        result.map_err(|e| self.fail(e.to_string())).is_ok()
    }

    /// Connect an output to an input. Returns "" on success, otherwise the reason.
    #[func]
    fn connect_ports(&mut self, from: GString, from_port: GString, to: GString, to_port: GString) -> GString {
        let result = lock(&self.shared).edit("Connect", None, |p| {
            p.graph.connect(
                registry(),
                &from.to_string(),
                &from_port.to_string(),
                &to.to_string(),
                &to_port.to_string(),
            )
        });
        match result {
            Ok(()) => GString::new(),
            Err(e) => e.to_string().as_str().into(),
        }
    }

    /// Remove the link into an input, if any.
    #[func]
    fn disconnect_input(&mut self, to: GString, to_port: GString) {
        let _ = lock(&self.shared).edit("Disconnect", None, |p| {
            p.graph.disconnect(&to.to_string(), &to_port.to_string());
            Ok(())
        });
    }

    /// Set a parameter. Returns the value actually stored (clamped to its
    /// range), or null on error. Rapid changes to the same parameter (e.g. a
    /// slider drag) merge into one undo step.
    #[func]
    fn set_param(&mut self, id: GString, key: GString, value: Variant) -> Variant {
        let Some(pv) = variant_to_param(&value) else {
            self.fail(format!("unsupported value type for '{key}'"));
            return Variant::nil();
        };
        let (id, key) = (id.to_string(), key.to_string());
        let label = {
            let s = lock(&self.shared);
            s.project
                .graph
                .node(&id)
                .and_then(|n| registry().schema(&n.type_id))
                .and_then(|sc| sc.param(&key))
                .map(|d| format!("Set {}", d.label))
                .unwrap_or_else(|| format!("Set {key}"))
        };
        let merge = format!("param:{id}:{key}");
        let result = lock(&self.shared).edit(&label, Some(&merge), |p| {
            p.graph.set_param(registry(), &id, &key, pv)
        });
        match result {
            Ok(v) => param_to_variant(&v),
            Err(e) => {
                self.fail(e.to_string());
                Variant::nil()
            }
        }
    }

    /// Show or hide a drivable parameter as an input port ("p:<key>").
    #[func]
    fn set_param_exposed(&mut self, id: GString, key: GString, exposed: bool) -> bool {
        let label = if exposed {
            "Show parameter port"
        } else {
            "Hide parameter port"
        };
        let result = lock(&self.shared).edit(label, None, |p| {
            p.graph
                .set_exposed(registry(), &id.to_string(), &key.to_string(), exposed)
        });
        result.map_err(|e| self.fail(e.to_string())).is_ok()
    }

    #[func]
    fn set_node_position(&mut self, id: GString, pos: Vector2) {
        let id = id.to_string();
        let merge = format!("move:{id}");
        let _ = lock(&self.shared).edit("Move node", Some(&merge), |p| {
            p.graph.set_position(&id, [pos.x, pos.y])
        });
    }

    /// The nearest heightfield output upstream of a node, as
    /// {"node": id, "port": key}; empty if there is none.
    #[func]
    fn get_base_heightfield(&self, id: GString) -> VarDictionary {
        let mut d = VarDictionary::new();
        if let Some((node, port)) = lock(&self.shared)
            .project
            .graph
            .base_heightfield(registry(), &id.to_string())
        {
            put(&mut d, "node", GString::from(node.as_str()));
            put(&mut d, "port", GString::from(port.as_str()));
        }
        d
    }

    /// Evaluate a curve (as stored by a Curve parameter) at `samples` evenly
    /// spaced x values from 0 to 1, exactly as the engine does. Empty if the
    /// points are invalid.
    #[func]
    fn eval_curve(points: PackedVector2Array, samples: i32) -> PackedFloat32Array {
        let value = variant_to_param(&points.to_variant());
        let Some(curve) = value.and_then(|v| terrain_core::Curve::parse(&v).ok()) else {
            return PackedFloat32Array::new();
        };
        let n = samples.max(2);
        (0..n)
            .map(|i| curve.eval(i as f64 / (n - 1) as f64) as f32)
            .collect()
    }

    /// Read a species preset file's text: `ok`, and `name`, `node` (the node
    /// type it is for), `biome` and `description`, or `error`.
    #[func]
    fn read_species_preset(text: GString) -> VarDictionary {
        let mut d = VarDictionary::new();
        match SpeciesPreset::parse(&text.to_string()) {
            Ok(p) => {
                put(&mut d, "ok", true);
                put(&mut d, "name", GString::from(p.name.as_str()));
                put(&mut d, "node", GString::from(p.node.as_str()));
                put(&mut d, "biome", GString::from(p.biome.as_str()));
                put(&mut d, "description", GString::from(p.description.as_str()));
            }
            Err(e) => {
                put(&mut d, "ok", false);
                put(&mut d, "error", GString::from(e.to_string().as_str()));
            }
        }
        d
    }

    /// Apply a species preset (a YAML file's text) to node `id` as one undo
    /// step. Returns `ok`, `name`, `warnings` (keys the node doesn't have,
    /// values it can't take) and, on failure, `error`.
    #[func]
    fn apply_species_preset(&mut self, id: GString, text: GString) -> VarDictionary {
        let mut d = VarDictionary::new();
        let id = id.to_string();
        let result = SpeciesPreset::parse(&text.to_string()).and_then(|preset| {
            let label = format!("Apply preset {}", preset.name);
            lock(&self.shared).edit(&label, None, |p| {
                let node = p
                    .graph
                    .node(&id)
                    .ok_or_else(|| terrain_core::CoreError::NodeNotFound(id.clone()))?;
                let schema = registry()
                    .schema(&node.type_id)
                    .ok_or_else(|| terrain_core::CoreError::UnknownNodeType(node.type_id.clone()))?;
                let mut warnings = Vec::new();
                if preset.node != schema.type_id {
                    warnings.push(format!(
                        "the preset is for {} nodes; settings this {} has were applied",
                        preset.node, schema.label
                    ));
                }
                let (values, more) = preset.values_for(schema);
                warnings.extend(more);
                for (key, value) in values {
                    p.graph.set_param(registry(), &id, &key, value)?;
                }
                Ok((preset.name.clone(), warnings))
            })
        });
        match result {
            Ok((name, warnings)) => {
                put(&mut d, "ok", true);
                put(&mut d, "name", GString::from(name.as_str()));
                let w: PackedStringArray = warnings.iter().map(|s| GString::from(s.as_str())).collect();
                put(&mut d, "warnings", w);
            }
            Err(e) => {
                self.fail(e.to_string());
                put(&mut d, "ok", false);
                put(&mut d, "error", GString::from(e.to_string().as_str()));
            }
        }
        d
    }

    /// Node `id`'s current settings as a species preset file (YAML), named
    /// `name`. Empty on error.
    #[func]
    fn make_species_preset(&mut self, id: GString, name: GString, biome: GString) -> GString {
        let s = lock(&self.shared);
        let Some(node) = s.project.graph.node(&id.to_string()) else {
            drop(s);
            self.fail(format!("no node {id}"));
            return GString::new();
        };
        let Some(schema) = registry().schema(&node.type_id) else {
            return GString::new();
        };
        SpeciesPreset::to_yaml(&name.to_string(), &biome.to_string(), schema, &node.params)
            .as_str()
            .into()
    }

    #[func]
    fn get_last_error(&self) -> GString {
        self.last_error.clone()
    }
}
