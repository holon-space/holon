use holon_api::render_types::{OperationParam, TypeHint};
use serde_json::Value;

use crate::mcp_sidecar::ParamOverride;

/// Convert a JSON Schema type + optional param override into a TypeHint.
pub fn json_schema_to_type_hint(
    schema: &serde_json::Map<String, Value>,
    param_override: Option<&ParamOverride>,
) -> TypeHint {
    if let Some(ovr) = param_override {
        if let Some(hint) = &ovr.type_hint {
            if let Some(entity) = hint.strip_prefix("entity_id:") {
                return TypeHint::EntityId {
                    entity_name: entity.into(),
                };
            }
        }
    }

    if let Some(Value::Array(variants)) = schema.get("enum") {
        return TypeHint::OneOf {
            values: variants
                .iter()
                .map(|v| holon_api::Value::from_json_value(v.clone()))
                .collect(),
        };
    }

    match schema.get("type").and_then(|v| v.as_str()) {
        Some("boolean") => TypeHint::Bool,
        Some("integer") | Some("number") => TypeHint::Number,
        Some("array") => TypeHint::String, // arrays serialized as JSON strings for now
        Some("object") => {
            match schema.get("properties").and_then(|v| v.as_object()) {
                Some(properties) => {
                    let required: Vec<&str> = schema
                        .get("required")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();

                    let fields = properties
                        .iter()
                        .map(|(name, prop_schema)| {
                            let prop = prop_schema.as_object().cloned().unwrap_or_default();
                            let description = prop
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let is_required = required.contains(&name.as_str());
                            let description = if is_required {
                                description
                            } else {
                                format!("{description} (optional)")
                            };
                            let type_hint = json_schema_to_type_hint(&prop, None);
                            OperationParam {
                                name: name.clone(),
                                type_hint,
                                description,
                            }
                        })
                        .collect();
                    TypeHint::Object { fields }
                }
                // Objects without properties → opaque JSON string
                None => TypeHint::String,
            }
        }
        _ => TypeHint::String,
    }
}

/// Convert an MCP tool's inputSchema into a list of OperationParams.
///
/// `required_names` lists param names that are required by the tool.
/// Params not in that list get `(optional)` appended to their description.
pub fn input_schema_to_params(
    input_schema: &serde_json::Map<String, Value>,
    param_overrides: Option<&std::collections::HashMap<String, ParamOverride>>,
) -> Vec<OperationParam> {
    let properties = match input_schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let required: Vec<&str> = input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    properties
        .iter()
        .map(|(name, prop_schema)| {
            let prop = prop_schema.as_object().cloned().unwrap_or_default();

            let description = prop
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let is_required = required.contains(&name.as_str());
            let description = if is_required {
                description
            } else {
                format!("{description} (optional)")
            };

            let override_for_param = param_overrides.and_then(|o| o.get(name));
            let type_hint = json_schema_to_type_hint(&prop, override_for_param);

            OperationParam {
                name: name.clone(),
                type_hint,
                description,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_type() {
        let schema = json!({"type": "string", "description": "task name"});
        let hint = json_schema_to_type_hint(schema.as_object().unwrap(), None);
        assert_eq!(hint, TypeHint::String);
    }

    #[test]
    fn boolean_type() {
        let schema = json!({"type": "boolean"});
        let hint = json_schema_to_type_hint(schema.as_object().unwrap(), None);
        assert_eq!(hint, TypeHint::Bool);
    }

    #[test]
    fn integer_type() {
        let schema = json!({"type": "integer"});
        let hint = json_schema_to_type_hint(schema.as_object().unwrap(), None);
        assert_eq!(hint, TypeHint::Number);
    }

    #[test]
    fn enum_type() {
        let schema = json!({"type": "string", "enum": ["p1", "p2", "p3", "p4"]});
        let hint = json_schema_to_type_hint(schema.as_object().unwrap(), None);
        match hint {
            TypeHint::OneOf { values } => assert_eq!(values.len(), 4),
            other => panic!("expected OneOf, got {:?}", other),
        }
    }

    #[test]
    fn entity_id_override() {
        let schema = json!({"type": "string"});
        let ovr = ParamOverride {
            type_hint: Some("entity_id:todoist_project".to_string()),
        };
        let hint = json_schema_to_type_hint(schema.as_object().unwrap(), Some(&ovr));
        assert_eq!(
            hint,
            TypeHint::EntityId {
                entity_name: "todoist_project".into()
            }
        );
    }

    #[test]
    fn input_schema_to_params_basic() {
        let schema = json!({
            "type": "object",
            "properties": {
                "ids": {"type": "array", "description": "Task IDs"},
                "content": {"type": "string", "description": "Task content"}
            },
            "required": ["ids"]
        });
        let params = input_schema_to_params(schema.as_object().unwrap(), None);
        assert_eq!(params.len(), 2);

        let ids_param = params.iter().find(|p| p.name == "ids").unwrap();
        assert!(!ids_param.description.contains("optional"));

        let content_param = params.iter().find(|p| p.name == "content").unwrap();
        assert!(content_param.description.contains("optional"));
    }
}
