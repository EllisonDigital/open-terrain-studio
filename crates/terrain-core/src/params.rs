use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// A stored parameter value. Serialised as a plain JSON value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Anything else (kept verbatim, e.g. for node types this version doesn't know).
    Other(serde_json::Value),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Float(v) => Some(*v),
            ParamValue::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ParamValue::Int(v) => Some(*v),
            ParamValue::Float(v) if v.fract() == 0.0 => Some(*v as i64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParamValue::Bool(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::Text(v) => Some(v),
            _ => None,
        }
    }
}

/// What kind of value a parameter holds, with its limits.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Float {
        min: f64,
        max: f64,
        /// Suggested slider step in the UI.
        step: f64,
    },
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    /// One of a fixed set of options: `(key, label)`.
    Enum {
        options: Vec<(String, String)>,
    },
}

/// Declaration of one node parameter. The Godot inspector is built from these.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamDef {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub default: ParamValue,
    /// Display unit, e.g. "m" or "°". Empty for unitless values.
    pub unit: String,
    pub description: String,
}

impl ParamDef {
    /// A float parameter.
    pub fn float(key: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        // A power of ten, so typed values like 0, 1.0 or 1200 are never snapped.
        let step = 10f64
            .powi(((max - min) / 1000.0).log10().floor() as i32)
            .clamp(0.001, 1.0);
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Float { min, max, step },
            default: ParamValue::Float(default),
            unit: String::new(),
            description: String::new(),
        }
    }

    /// A float parameter in metres.
    pub fn metres(key: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        let mut p = Self::float(key, label, default, min, max).unit("m");
        p.kind = ParamKind::Float { min, max, step: 1.0 };
        p
    }

    pub fn int(key: &str, label: &str, default: i64, min: i64, max: i64) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Int { min, max },
            default: ParamValue::Int(default),
            unit: String::new(),
            description: String::new(),
        }
    }

    pub fn bool(key: &str, label: &str, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Bool,
            default: ParamValue::Bool(default),
            unit: String::new(),
            description: String::new(),
        }
    }

    pub fn choice(key: &str, label: &str, default: &str, options: &[(&str, &str)]) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Enum {
                options: options
                    .iter()
                    .map(|(k, l)| (k.to_string(), l.to_string()))
                    .collect(),
            },
            default: ParamValue::Text(default.into()),
            unit: String::new(),
            description: String::new(),
        }
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.into();
        self
    }

    pub fn describe(mut self, text: &str) -> Self {
        self.description = text.into();
        self
    }

    /// Coerce and clamp a value to this parameter's kind and limits.
    pub fn validate(&self, value: &ParamValue) -> Result<ParamValue> {
        let bad = |reason: &str| CoreError::InvalidParam {
            param: self.key.clone(),
            reason: reason.into(),
        };
        match &self.kind {
            ParamKind::Float { min, max, .. } => {
                let v = value.as_f64().ok_or_else(|| bad("expected a number"))?;
                if !v.is_finite() {
                    return Err(bad("must be finite"));
                }
                Ok(ParamValue::Float(v.clamp(*min, *max)))
            }
            ParamKind::Int { min, max } => {
                let v = value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|f| f.round() as i64))
                    .ok_or_else(|| bad("expected a whole number"))?;
                Ok(ParamValue::Int(v.clamp(*min, *max)))
            }
            ParamKind::Bool => value
                .as_bool()
                .map(ParamValue::Bool)
                .ok_or_else(|| bad("expected true or false")),
            ParamKind::Enum { options } => {
                let s = value.as_str().ok_or_else(|| bad("expected an option name"))?;
                if options.iter().any(|(k, _)| k == s) {
                    Ok(ParamValue::Text(s.into()))
                } else {
                    Err(bad(&format!("unknown option '{s}'")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_is_plain_values() {
        let v: Vec<ParamValue> = serde_json::from_str(r#"[true, 3, 2.5, "perlin"]"#).unwrap();
        assert_eq!(
            v,
            vec![
                ParamValue::Bool(true),
                ParamValue::Int(3),
                ParamValue::Float(2.5),
                ParamValue::Text("perlin".into())
            ]
        );
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"[true,3,2.5,"perlin"]"#);
    }

    #[test]
    fn validate_coerces_and_clamps() {
        let p = ParamDef::metres("size", "Size", 100.0, 1.0, 1000.0);
        assert_eq!(
            p.validate(&ParamValue::Int(800)).unwrap(),
            ParamValue::Float(800.0)
        );
        assert_eq!(
            p.validate(&ParamValue::Float(5000.0)).unwrap(),
            ParamValue::Float(1000.0)
        );
        assert!(p.validate(&ParamValue::Text("x".into())).is_err());

        let e = ParamDef::choice("mode", "Mode", "add", &[("add", "Add"), ("max", "Max")]);
        assert!(e.validate(&ParamValue::Text("max".into())).is_ok());
        assert!(e.validate(&ParamValue::Text("nope".into())).is_err());
    }
}
