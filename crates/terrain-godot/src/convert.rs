//! Conversions between engine values and Godot Variants.

use godot::prelude::*;
use terrain_core::ParamValue;

/// Convert any JSON value into a Godot Variant (objects -> Dictionary, arrays -> Array).
pub fn json_to_variant(v: &serde_json::Value) -> Variant {
    match v {
        serde_json::Value::Null => Variant::nil(),
        serde_json::Value::Bool(b) => b.to_variant(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_variant()
            } else {
                n.as_f64().unwrap_or(0.0).to_variant()
            }
        }
        serde_json::Value::String(s) => GString::from(s.as_str()).to_variant(),
        serde_json::Value::Array(items) => {
            let mut arr = VarArray::new();
            for item in items {
                arr.push(&json_to_variant(item));
            }
            arr.to_variant()
        }
        serde_json::Value::Object(map) => {
            let mut dict = VarDictionary::new();
            for (k, item) in map {
                dict.set(&GString::from(k.as_str()).to_variant(), &json_to_variant(item));
            }
            dict.to_variant()
        }
    }
}

pub fn param_to_variant(v: &ParamValue) -> Variant {
    match v {
        ParamValue::Bool(b) => b.to_variant(),
        ParamValue::Int(i) => i.to_variant(),
        ParamValue::Float(f) => f.to_variant(),
        ParamValue::Text(s) => GString::from(s.as_str()).to_variant(),
        ParamValue::Other(j) => json_to_variant(j),
    }
}

pub fn variant_to_param(v: &Variant) -> Option<ParamValue> {
    match v.get_type() {
        VariantType::BOOL => v.try_to::<bool>().ok().map(ParamValue::Bool),
        VariantType::INT => v.try_to::<i64>().ok().map(ParamValue::Int),
        VariantType::FLOAT => v.try_to::<f64>().ok().map(ParamValue::Float),
        VariantType::STRING | VariantType::STRING_NAME => v
            .try_to::<GString>()
            .ok()
            .map(|s| ParamValue::Text(s.to_string())),
        _ => None,
    }
}

/// Helper: set `dict[key] = value`.
pub fn put(dict: &mut VarDictionary, key: &str, value: impl ToGodot) {
    dict.set(&GString::from(key).to_variant(), &value.to_variant());
}
