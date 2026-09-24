use godot::prelude::*;
use terrain_core::NodeSchema;

use crate::convert::{json_to_variant, param_to_variant, put, variant_to_param};
use crate::project::{Shared, lock};
use crate::registry;

/// The node graph of a `TerrainProject`. A live view: every edit goes straight
/// into the project owned by Rust (Godot never holds the source of truth).
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
}

#[godot_api]
impl TerrainGraph {
    /// Every available node type: type_id, label, category, description,
    /// inputs, outputs and params (key, label, kind, min, max, step, options,
    /// default, unit, description).
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

    /// All nodes: id, type, label, category, pos (Vector2), known (bool),
    /// inputs, outputs, params (effective values, defaults filled in).
    #[func]
    fn get_nodes(&self) -> VarArray {
        let s = lock(&self.shared);
        let mut arr = VarArray::new();
        for node in s.project.graph.nodes() {
            let mut d = VarDictionary::new();
            put(&mut d, "id", GString::from(node.id.as_str()));
            put(&mut d, "type", GString::from(node.type_id.as_str()));
            put(&mut d, "pos", Vector2::new(node.pos[0], node.pos[1]));
            let mut params = VarDictionary::new();
            match registry().schema(&node.type_id) {
                Some(schema) => {
                    put(&mut d, "known", true);
                    put(&mut d, "label", GString::from(schema.label.as_str()));
                    put(&mut d, "category", GString::from(schema.category.as_str()));
                    let full = serde_json::to_value(schema).unwrap_or_default();
                    d.set(&"inputs".to_variant(), &json_to_variant(&full["inputs"]));
                    d.set(&"outputs".to_variant(), &json_to_variant(&full["outputs"]));
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
                    put(&mut d, "inputs", VarArray::new());
                    put(&mut d, "outputs", VarArray::new());
                    for (k, v) in &node.params {
                        params.set(&GString::from(k.as_str()).to_variant(), &param_to_variant(v));
                    }
                }
            }
            put(&mut d, "params", params);
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
        let result = {
            let mut s = lock(&self.shared);
            let r = s
                .project
                .graph
                .add_node(registry(), &type_id.to_string(), [pos.x, pos.y]);
            if r.is_ok() {
                s.modified = true;
            }
            r
        };
        match result {
            Ok(id) => id.as_str().into(),
            Err(e) => {
                self.fail(e.to_string());
                GString::new()
            }
        }
    }

    #[func]
    fn remove_node(&mut self, id: GString) -> bool {
        let result = {
            let mut s = lock(&self.shared);
            let r = s.project.graph.remove_node(&id.to_string());
            if r.is_ok() {
                s.modified = true;
            }
            r
        };
        result.map_err(|e| self.fail(e.to_string())).is_ok()
    }

    /// Connect an output to an input. Returns "" on success, otherwise the reason.
    #[func]
    fn connect_ports(&mut self, from: GString, from_port: GString, to: GString, to_port: GString) -> GString {
        let mut s = lock(&self.shared);
        match s.project.graph.connect(
            registry(),
            &from.to_string(),
            &from_port.to_string(),
            &to.to_string(),
            &to_port.to_string(),
        ) {
            Ok(()) => {
                s.modified = true;
                GString::new()
            }
            Err(e) => e.to_string().as_str().into(),
        }
    }

    /// Remove the link into an input, if any.
    #[func]
    fn disconnect_input(&mut self, to: GString, to_port: GString) {
        let mut s = lock(&self.shared);
        s.project.graph.disconnect(&to.to_string(), &to_port.to_string());
        s.modified = true;
    }

    /// Set a parameter. Returns the value actually stored (clamped to its
    /// range), or null on error.
    #[func]
    fn set_param(&mut self, id: GString, key: GString, value: Variant) -> Variant {
        let Some(pv) = variant_to_param(&value) else {
            self.fail(format!("unsupported value type for '{key}'"));
            return Variant::nil();
        };
        let result = {
            let mut s = lock(&self.shared);
            let r = s
                .project
                .graph
                .set_param(registry(), &id.to_string(), &key.to_string(), pv);
            if r.is_ok() {
                s.modified = true;
            }
            r
        };
        match result {
            Ok(v) => param_to_variant(&v),
            Err(e) => {
                self.fail(e.to_string());
                Variant::nil()
            }
        }
    }

    #[func]
    fn set_node_position(&mut self, id: GString, pos: Vector2) {
        let mut s = lock(&self.shared);
        if s.project
            .graph
            .set_position(&id.to_string(), [pos.x, pos.y])
            .is_ok()
        {
            s.modified = true;
        }
    }

    #[func]
    fn get_last_error(&self) -> GString {
        self.last_error.clone()
    }
}
