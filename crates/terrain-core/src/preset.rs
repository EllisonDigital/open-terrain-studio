//! Species presets (ARCHITECTURE.md §8): saved parameter sets for a
//! vegetation node, stored as small YAML files such as `scots_pine.yaml`.
//!
//! A preset is a flat list of `key: value` lines. `name`, `node` (the node
//! type it is for), `biome` and `description` describe the preset; every
//! other key is a parameter of that node. Only this flat subset of YAML is
//! read: numbers, `true`/`false`, and plain or quoted strings, with `#`
//! comments. Nested maps and lists are rejected.

use std::collections::BTreeMap;

use crate::error::{CoreError, Result};
use crate::node::NodeSchema;
use crate::params::{ParamKind, ParamValue};

/// Keys that describe a preset rather than set a parameter.
const META_KEYS: [&str; 4] = ["name", "node", "biome", "description"];

/// A parsed species preset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeciesPreset {
    /// Display name, e.g. "Scots pine".
    pub name: String,
    /// Node type the preset is for, e.g. `vegetation.trees`.
    pub node: String,
    /// e.g. "temperate", "alpine", "desert" or "tropical".
    pub biome: String,
    pub description: String,
    /// Parameter values, as written in the file.
    pub params: BTreeMap<String, ParamValue>,
}

fn preset_error(line: usize, message: &str) -> CoreError {
    CoreError::Preset(format!("line {line}: {message}"))
}

/// Parse one scalar: quoted string, bool, integer, float or plain string.
fn parse_scalar(raw: &str, line: usize) -> Result<ParamValue> {
    let raw = raw.trim();
    if let Some(q) = raw.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let rest = &raw[1..];
        let end = rest
            .find(q)
            .ok_or_else(|| preset_error(line, "unterminated quoted string"))?;
        let after = rest[end + 1..].trim();
        if !(after.is_empty() || after.starts_with('#')) {
            return Err(preset_error(line, "unexpected text after a quoted string"));
        }
        let text = &rest[..end];
        return Ok(ParamValue::Text(if q == '"' {
            text.replace("\\\"", "\"")
        } else {
            text.to_string()
        }));
    }
    // Plain scalars end at a comment.
    let plain = match raw.find(" #") {
        Some(i) => raw[..i].trim_end(),
        None => raw,
    };
    if plain.is_empty() {
        return Err(preset_error(
            line,
            "missing value (nested maps and lists aren't supported)",
        ));
    }
    if plain.starts_with(['[', '{', '-', '&', '*', '|', '>']) && plain.parse::<f64>().is_err() {
        return Err(preset_error(
            line,
            "only plain values are supported, not lists or maps",
        ));
    }
    Ok(match plain {
        "true" | "True" | "TRUE" => ParamValue::Bool(true),
        "false" | "False" | "FALSE" => ParamValue::Bool(false),
        _ => {
            if let Ok(i) = plain.parse::<i64>() {
                ParamValue::Int(i)
            } else if let Ok(f) = plain
                .parse::<f64>()
                .map_err(|_| ())
                .and_then(|f| if f.is_finite() { Ok(f) } else { Err(()) })
            {
                ParamValue::Float(f)
            } else {
                ParamValue::Text(plain.to_string())
            }
        }
    })
}

impl SpeciesPreset {
    /// Parse a preset file's text.
    pub fn parse(text: &str) -> Result<Self> {
        let mut preset = SpeciesPreset::default();
        for (n, line) in text.lines().enumerate() {
            let line_no = n + 1;
            let trimmed = line.trim_end();
            if trimmed.trim_start().is_empty() || trimmed.trim_start().starts_with('#') || trimmed == "---" {
                continue;
            }
            if trimmed.starts_with([' ', '\t']) {
                return Err(preset_error(
                    line_no,
                    "indented lines (nested values) aren't supported",
                ));
            }
            let (key, value) = trimmed
                .split_once(':')
                .ok_or_else(|| preset_error(line_no, "expected 'key: value'"))?;
            let key = key.trim();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(preset_error(line_no, &format!("invalid key '{key}'")));
            }
            let value = parse_scalar(value, line_no)?;
            if META_KEYS.contains(&key) {
                let text = match &value {
                    ParamValue::Text(s) => s.clone(),
                    ParamValue::Int(i) => i.to_string(),
                    ParamValue::Float(f) => f.to_string(),
                    ParamValue::Bool(b) => b.to_string(),
                    ParamValue::Other(v) => v.to_string(),
                };
                match key {
                    "name" => preset.name = text,
                    "node" => preset.node = text,
                    "biome" => preset.biome = text,
                    _ => preset.description = text,
                }
            } else if preset.params.insert(key.to_string(), value).is_some() {
                return Err(preset_error(line_no, &format!("'{key}' is set twice")));
            }
        }
        if preset.node.is_empty() {
            return Err(CoreError::Preset(
                "'node' (the node type it is for) is missing".into(),
            ));
        }
        Ok(preset)
    }

    /// The preset's parameters, checked against a node schema: values
    /// coerced and clamped to each parameter's range, in schema order.
    /// Also returns a warning for each key the node doesn't have.
    pub fn values_for(&self, schema: &NodeSchema) -> (Vec<(String, ParamValue)>, Vec<String>) {
        let mut values = Vec::new();
        let mut warnings = Vec::new();
        for def in &schema.params {
            if let Some(v) = self.params.get(&def.key) {
                match def.validate(v) {
                    Ok(v) => values.push((def.key.clone(), v)),
                    Err(e) => warnings.push(e.to_string()),
                }
            }
        }
        for key in self.params.keys() {
            if schema.param(key).is_none() {
                warnings.push(format!("'{}' has no parameter '{key}'", schema.label));
            }
        }
        (values, warnings)
    }

    /// A preset file for a node's current settings (every parameter except
    /// the seed, in schema order).
    pub fn to_yaml(
        name: &str,
        biome: &str,
        schema: &NodeSchema,
        params: &BTreeMap<String, ParamValue>,
    ) -> String {
        let mut out = String::from("# OpenTerrainStudio species preset\n");
        out += &format!("name: {}\n", yaml_string(name));
        out += &format!("node: {}\n", schema.type_id);
        if !biome.is_empty() {
            out += &format!("biome: {}\n", yaml_string(biome));
        }
        for def in schema.params.iter().filter(|d| d.key != "seed") {
            let v = params
                .get(&def.key)
                .and_then(|v| def.validate(v).ok())
                .unwrap_or_else(|| def.default.clone());
            let text = match (&def.kind, &v) {
                (_, ParamValue::Bool(b)) => b.to_string(),
                (_, ParamValue::Int(i)) => i.to_string(),
                (_, ParamValue::Float(f)) => format_float(*f),
                (ParamKind::Enum { .. } | ParamKind::Text | ParamKind::File { .. }, ParamValue::Text(s)) => {
                    yaml_string(s)
                }
                // Curves and gradients don't fit the flat format.
                _ => continue,
            };
            out += &format!("{}: {text}\n", def.key);
        }
        out
    }
}

fn format_float(f: f64) -> String {
    let s = format!("{:.4}", f);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.into()
    }
}

/// A string as a YAML scalar, quoted when a plain one would read differently.
fn yaml_string(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' ' | '.' | '(' | ')' | ','))
        && !s.starts_with(['-', ' '])
        && !s.ends_with(' ')
        && !matches!(s, "true" | "false" | "True" | "False" | "TRUE" | "FALSE")
        && s.parse::<f64>().is_err();
    if plain {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeSchema, PortDef, PortType};
    use crate::params::ParamDef;

    fn schema() -> NodeSchema {
        NodeSchema {
            type_id: "vegetation.trees".into(),
            type_version: 1,
            label: "Trees".into(),
            category: "Vegetation".into(),
            description: String::new(),
            inputs: vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
            outputs: vec![],
            params: vec![
                ParamDef::text("species", "Species", "tree"),
                ParamDef::metres("spacing_m", "Spacing", 6.0, 0.5, 1000.0),
                ParamDef::float("health", "Health", 0.8, 0.0, 1.0),
                ParamDef::bool("flag", "Flag", false),
                ParamDef::choice("mode", "Mode", "avoid", &[("avoid", "Avoid"), ("near", "Near")]),
                ParamDef::int("seed", "Seed", 0, 0, 999_999),
            ],
            gpu: false,
        }
    }

    #[test]
    fn parses_the_flat_subset() {
        let p = SpeciesPreset::parse(
            "# comment\nname: Scots pine\nnode: vegetation.trees\nbiome: temperate\n\
             species: scots_pine   # trailing comment\nspacing_m: 7\nhealth: 0.75\nflag: true\n\
             mode: \"near\"\n",
        )
        .unwrap();
        assert_eq!(p.name, "Scots pine");
        assert_eq!(p.node, "vegetation.trees");
        assert_eq!(p.params["species"], ParamValue::Text("scots_pine".into()));
        assert_eq!(p.params["spacing_m"], ParamValue::Int(7));
        assert_eq!(p.params["health"], ParamValue::Float(0.75));
        assert_eq!(p.params["flag"], ParamValue::Bool(true));
        let (values, warnings) = p.values_for(&schema());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(values[1], ("spacing_m".into(), ParamValue::Float(7.0)));
        assert_eq!(values[4], ("mode".into(), ParamValue::Text("near".into())));
    }

    #[test]
    fn rejects_nested_yaml_and_reports_unknown_keys() {
        assert!(SpeciesPreset::parse("node: x\nlist:\n  - a\n").is_err());
        assert!(SpeciesPreset::parse("node: x\nlist: [1, 2]\n").is_err());
        assert!(SpeciesPreset::parse("name: no node\n").is_err());
        assert!(SpeciesPreset::parse("node: x\na: 1\na: 2\n").is_err());
        let p = SpeciesPreset::parse("node: vegetation.trees\nheight_of_trunk: 3\nhealth: 7\n").unwrap();
        let (values, warnings) = p.values_for(&schema());
        assert_eq!(values, vec![("health".into(), ParamValue::Float(1.0))]);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn saved_presets_read_back() {
        let s = schema();
        let mut params = BTreeMap::new();
        params.insert("species".into(), ParamValue::Text("true".into()));
        params.insert("spacing_m".into(), ParamValue::Float(12.5));
        params.insert("seed".into(), ParamValue::Int(42));
        let yaml = SpeciesPreset::to_yaml("My: pine", "alpine", &s, &params);
        let p = SpeciesPreset::parse(&yaml).unwrap();
        assert_eq!(p.name, "My: pine");
        assert_eq!(p.biome, "alpine");
        assert_eq!(p.params["species"], ParamValue::Text("true".into()));
        assert_eq!(p.params["spacing_m"], ParamValue::Float(12.5));
        assert_eq!(p.params["health"], ParamValue::Float(0.8));
        assert!(!p.params.contains_key("seed"));
    }
}
