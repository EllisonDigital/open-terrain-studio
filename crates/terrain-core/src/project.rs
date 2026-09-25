//! The `.otstudio` project file: human-readable JSON, versioned from day one,
//! with no computed data inside it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::graph::{Graph, Link, NodeInstance};
use crate::node::NodeRegistry;
use crate::params::ParamValue;
use crate::world::World;

/// Current project file format version.
pub const FORMAT_VERSION: u32 = 1;

/// File extension for projects (without the dot).
pub const EXTENSION: &str = "otstudio";

/// An output marked for export in one format. An output exported in two
/// formats has two entries.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExportSpec {
    pub node: String,
    pub port: String,
    pub format: String,
}

/// Build settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildSettings {
    pub resolution: u32,
    pub folder: String,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            resolution: 2048,
            folder: "output/".into(),
        }
    }
}

/// A whole project: world, graph, export marks, build settings and UI state.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub world: World,
    pub graph: Graph,
    pub exports: Vec<ExportSpec>,
    pub build: BuildSettings,
    /// Editor state (viewed node, camera, ...). Opaque to the engine.
    pub ui: serde_json::Value,
}

impl Project {
    /// Remove a node, its links and its export marks.
    pub fn remove_node(&mut self, id: &str) -> Result<()> {
        self.graph.remove_node(id)?;
        self.exports.retain(|e| e.node != id);
        Ok(())
    }

    /// Mark or unmark `node.port` for export in `format` (e.g. "exr32").
    pub fn set_export(&mut self, node: &str, port: &str, format: &str, on: bool) -> Result<()> {
        if crate::export::ExportFormat::parse(format).is_none() {
            return Err(CoreError::Project(format!("unknown export format '{format}'")));
        }
        if self.graph.node(node).is_none() {
            return Err(CoreError::NodeNotFound(node.into()));
        }
        let spec = ExportSpec {
            node: node.into(),
            port: port.into(),
            format: format.into(),
        };
        self.exports.retain(|e| e != &spec);
        if on {
            self.exports.push(spec);
            self.exports.sort();
        }
        Ok(())
    }

    /// Formats `node.port` is marked for.
    pub fn export_formats(&self, node: &str, port: &str) -> Vec<String> {
        self.exports
            .iter()
            .filter(|e| e.node == node && e.port == port)
            .map(|e| e.format.clone())
            .collect()
    }
}

impl Default for Project {
    fn default() -> Self {
        Self {
            world: World::default(),
            graph: Graph::new(),
            exports: Vec::new(),
            build: BuildSettings::default(),
            ui: serde_json::json!({}),
        }
    }
}

// ---- on-disk representation ------------------------------------------------

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format_version: u32,
    app_version: String,
    world: World,
    #[serde(default)]
    next_node_id: u64,
    #[serde(default)]
    nodes: Vec<NodeFile>,
    #[serde(default)]
    links: Vec<LinkFile>,
    #[serde(default)]
    exports: Vec<ExportSpec>,
    #[serde(default)]
    build: BuildSettings,
    #[serde(default)]
    ui: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct NodeFile {
    id: String,
    #[serde(rename = "type")]
    type_id: String,
    type_version: u32,
    pos: [f32; 2],
    #[serde(default)]
    params: BTreeMap<String, ParamValue>,
    /// Parameters shown as input ports.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    exposed: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct LinkFile {
    from: [String; 2],
    to: [String; 2],
}

impl Project {
    /// Serialise to pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        let file = ProjectFile {
            format_version: FORMAT_VERSION,
            app_version: crate::APP_VERSION.into(),
            world: self.world.clone(),
            next_node_id: self.graph.next_id(),
            nodes: self
                .graph
                .nodes()
                .map(|n| NodeFile {
                    id: n.id.clone(),
                    type_id: n.type_id.clone(),
                    type_version: n.type_version,
                    pos: n.pos,
                    params: n.params.clone(),
                    exposed: n.exposed.clone(),
                })
                .collect(),
            links: self
                .graph
                .links()
                .iter()
                .map(|l| LinkFile {
                    from: [l.from.0.clone(), l.from.1.clone()],
                    to: [l.to.0.clone(), l.to.1.clone()],
                })
                .collect(),
            exports: self.exports.clone(),
            build: self.build.clone(),
            ui: self.ui.clone(),
        };
        Ok(serde_json::to_string_pretty(&file)? + "\n")
    }

    /// Parse a project. Returns the project plus human-readable warnings
    /// (unknown node types, dropped links, migrated nodes).
    ///
    /// Unknown node types are **kept** (so saving again loses nothing); they just
    /// can't be evaluated until a version that knows them opens the file.
    pub fn from_json(json: &str, registry: &NodeRegistry) -> Result<(Project, Vec<String>)> {
        let file: ProjectFile = serde_json::from_str(json)?;
        if file.format_version > FORMAT_VERSION {
            return Err(CoreError::Project(format!(
                "this project was saved by a newer OpenTerrainStudio (format {}, this version reads up to {FORMAT_VERSION})",
                file.format_version
            )));
        }
        file.world.validate().map_err(CoreError::Project)?;

        let mut warnings = Vec::new();
        let mut graph = Graph::new();
        for n in file.nodes {
            let mut node = NodeInstance {
                id: n.id,
                type_id: n.type_id,
                type_version: n.type_version,
                pos: n.pos,
                params: n.params,
                exposed: n.exposed,
            };
            match registry.get(&node.type_id) {
                None => warnings.push(format!(
                    "node {} has unknown type '{}' and was kept as a placeholder",
                    node.id, node.type_id
                )),
                Some(kind) => {
                    let current = kind.schema().type_version;
                    if node.type_version < current {
                        kind.migrate(node.type_version, &mut node.params)?;
                        warnings.push(format!(
                            "node {} upgraded from version {} to {current}",
                            node.id, node.type_version
                        ));
                        node.type_version = current;
                    } else if node.type_version > current {
                        warnings.push(format!(
                            "node {} was saved by a newer version of '{}'; results may differ",
                            node.id, node.type_id
                        ));
                    }
                    // Drop parameters this version doesn't know (keep the rest).
                    let schema = kind.schema();
                    node.params.retain(|k, v| match schema.param(k) {
                        Some(def) => {
                            if let Ok(valid) = def.validate(v) {
                                *v = valid;
                                true
                            } else {
                                false
                            }
                        }
                        None => false,
                    });
                    node.exposed
                        .retain(|k| schema.param(k).is_some_and(|d| d.drivable));
                }
            }
            graph.insert_raw(node);
        }
        graph.next_id = graph.next_id.max(file.next_node_id);

        for l in file.links {
            let link = Link {
                from: (l.from[0].clone(), l.from[1].clone()),
                to: (l.to[0].clone(), l.to[1].clone()),
            };
            let both_known = [&link.from.0, &link.to.0]
                .iter()
                .all(|id| graph.node(id).is_some_and(|n| registry.get(&n.type_id).is_some()));
            if both_known {
                if let Err(e) = graph.connect(registry, &link.from.0, &link.from.1, &link.to.0, &link.to.1) {
                    warnings.push(format!(
                        "dropped link {}.{} -> {}.{}: {e}",
                        link.from.0, link.from.1, link.to.0, link.to.1
                    ));
                }
            } else if graph.node(&link.from.0).is_some() && graph.node(&link.to.0).is_some() {
                // Keep links to placeholder nodes untouched.
                graph.links.push(link);
                graph.links.sort();
            } else {
                warnings.push(format!(
                    "dropped link to missing node {} or {}",
                    link.from.0, link.to.0
                ));
            }
        }

        let mut exports = file.exports;
        exports.retain(|e| {
            let keep = graph.node(&e.node).is_some();
            if !keep {
                warnings.push(format!("dropped export of missing node {}", e.node));
            }
            keep
        });
        exports.sort();
        exports.dedup();

        Ok((
            Project {
                world: file.world,
                graph,
                exports,
                build: file.build,
                ui: if file.ui.is_null() {
                    serde_json::json!({})
                } else {
                    file.ui
                },
            },
            warnings,
        ))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        // Write to a temporary file first so a crash never leaves a half-written project.
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path, registry: &NodeRegistry) -> Result<(Project, Vec<String>)> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json, registry)
    }
}
