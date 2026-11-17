//! A recipe's front-matter value as the single text a document property holds.
//!
//! Scalars keep their own text, a list of scalars joins with `", "`, and a
//! nested value (a map, or a list holding one) becomes canonical JSON: source
//! key order, integers stay integers, floats stay floats. Anything JSON cannot
//! hold losslessly is refused by name with its path.

use serde_yaml::Value as Yaml;

pub fn value_text(key: &str, value: &Yaml) -> Result<String, String> {
    match value {
        Yaml::String(s) => Ok(s.clone()),
        Yaml::Number(n) => Ok(n.to_string()),
        Yaml::Bool(b) => Ok(b.to_string()),
        Yaml::Sequence(items) if !items.iter().any(is_container) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Yaml::String(s) => parts.push(s.clone()),
                    Yaml::Number(n) => parts.push(n.to_string()),
                    Yaml::Bool(b) => parts.push(b.to_string()),
                    other => {
                        return Err(format!(
                            "cooklang metadata {key:?}: list entry {other:?} is neither a scalar \
                             nor a nested value"
                        ));
                    }
                }
            }
            Ok(parts.join(", "))
        }
        Yaml::Sequence(_) | Yaml::Mapping(_) => {
            let mut out = String::new();
            write_json(&mut key.to_string(), value, &mut out)
                .map_err(|e| format!("cooklang metadata {key:?}: {e}"))?;
            Ok(out)
        }
        other => Err(format!(
            "cooklang metadata {key:?}: value {other:?} is neither a scalar nor a list nor a map"
        )),
    }
}

/// The YAML value a property written by [`value_text`] stands for. A `.cook`
/// writer emits a nested value as block YAML built from this, never as the
/// JSON text.
#[allow(dead_code)] // no writer exists yet; the round-trip test is its caller
pub fn yaml_from_property_text(text: &str) -> Result<Yaml, String> {
    serde_json::from_str(text).map_err(|e| format!("property text is not JSON: {e}"))
}

fn is_container(value: &Yaml) -> bool {
    matches!(value, Yaml::Sequence(_) | Yaml::Mapping(_))
}

/// `path` names the value for the refusal message: `graph.entities[0].id`.
fn write_json(path: &mut String, value: &Yaml, out: &mut String) -> Result<(), String> {
    match value {
        Yaml::Null => out.push_str("null"),
        Yaml::Bool(b) => out.push_str(&b.to_string()),
        Yaml::Number(n) => {
            if n.is_nan() || n.is_infinite() {
                return Err(format!("{path} is {n}, which JSON cannot hold"));
            }
            out.push_str(&serde_json::to_string(n).map_err(|e| format!("{path}: {e}"))?);
        }
        Yaml::String(s) => push_string(s, out),
        Yaml::Sequence(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let len = path.len();
                path.push_str(&format!("[{index}]"));
                write_json(path, item, out)?;
                path.truncate(len);
            }
            out.push(']');
        }
        Yaml::Mapping(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                let Yaml::String(name) = key else {
                    return Err(format!(
                        "map key {key:?} under {path} is not a string and JSON cannot hold it"
                    ));
                };
                if index > 0 {
                    out.push(',');
                }
                push_string(name, out);
                out.push(':');
                let len = path.len();
                path.push('.');
                path.push_str(name);
                write_json(path, item, out)?;
                path.truncate(len);
            }
            out.push('}');
        }
        Yaml::Tagged(tagged) => {
            return Err(format!(
                "{path} carries the YAML tag {} and JSON cannot hold it",
                tagged.tag
            ));
        }
    }
    Ok(())
}

fn push_string(s: &str, out: &mut String) {
    out.push_str(&serde_json::to_string(s).expect("a string always serializes"));
}
