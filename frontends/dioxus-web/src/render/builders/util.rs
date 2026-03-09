//! Shared display helpers for builder files.

use holon_api::Value;

/// Format a `holon_api::Value` as a human-readable string.
pub(crate) fn value_to_display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => s.clone(),
        Value::DateTime(s) => s.clone(),
        Value::Json(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .map(value_to_display)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            let mut pairs: Vec<(&String, &Value)> = map.iter().collect();
            pairs.sort_by_key(|(k, _)| k.as_str());
            pairs
                .iter()
                .map(|(k, v)| format!("{k}: {}", value_to_display(v)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}
