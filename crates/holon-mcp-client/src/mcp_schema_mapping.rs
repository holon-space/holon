use holon_api::render_types::OperationParam;
use holon_api::render_types::TypeHint;
use serde_json::Value;

use crate::mcp_sidecar::ParamOverride;

/// Convert a JSON Schema type + optional param override into a TypeHint.
pub fn json_schema_to_type_hint(
    schema: &serde_json::Map<String, Value>,
    param_override: Option<&ParamOverride>,
) -> TypeHint {
    if let Some(ovr) = param_override
        && let Some(hint) = &ovr.type_hint
        && let Some(entity) = hint.strip_prefix("entity_id:")
    {
        return TypeHint::EntityId {
            entity_name: entity.into(),
        };
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
            let type_hint = connector_param_hint(
                name,
                &description,
                json_schema_to_type_hint(&prop, override_for_param),
            );

            OperationParam {
                name: name.clone(),
                type_hint,
                description,
            }
        })
        .collect()
}

/// The hint a parameter synthesized from a CONNECTOR's tool schema declares.
///
/// The value is a key in the connector's own vocabulary — it crosses to the
/// external system verbatim (`"inbox"`, a Todoist project id, an email) — so a
/// parameter the registration rule reads as an undeclared reference is not a
/// Holon declaration that forgot its entity, and `RowKey` is what the boundary
/// should read it as. The rule's own [`holon_api::id_like_but_undeclared`]
/// decides which parameters those are, so a shape the rule refuses is a shape
/// this converts whatever its schema type; a sidecar that DOES mean a Holon
/// entity reference says so with `param_overrides` (`entity_id:<entity>`),
/// which arrives already parsed and passes through.
fn connector_param_hint(name: &str, description: &str, hint: TypeHint) -> TypeHint {
    if holon_api::id_like_but_undeclared(name, description, &hint) {
        return TypeHint::RowKey;
    }
    hint
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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

    /// A connector's id-shaped parameters declare their own-vocabulary role,
    /// `RowKey` — declared `String`, the operation boundary's gate reads them
    /// as Holon entity references and refuses the whole provider at
    /// registration. `content` is the control: a value param keeps its plain
    /// `String`.
    #[test]
    fn a_connector_id_param_is_declared_its_row_key() {
        let schema = json!({
            "type": "object",
            "properties": {
                "projectId": {"type": "string", "description": "The ID of the project."},
                "content": {"type": "string", "description": "The task content"},
            },
            "required": ["projectId"]
        });
        let params = input_schema_to_params(schema.as_object().unwrap(), None);
        let hint = |name: &str| {
            params
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("no param {name}"))
                .type_hint
                .clone()
        };
        assert_eq!(hint("projectId"), TypeHint::RowKey);
        assert_eq!(hint("content"), TypeHint::String);
    }

    /// The sidecar's own statement wins: a `param_overrides` entry that names a
    /// Holon entity is what the boundary parses with, and the connector-role
    /// default must not overwrite it.
    #[test]
    fn an_entity_id_override_outranks_the_connector_role() {
        let schema = json!({
            "type": "object",
            "properties": {"projectId": {"type": "string", "description": "The ID of the project."}},
            "required": ["projectId"]
        });
        let mut overrides = std::collections::HashMap::new();
        overrides.insert(
            "projectId".to_string(),
            ParamOverride {
                type_hint: Some("entity_id:todoist_projects".to_string()),
            },
        );
        let params = input_schema_to_params(schema.as_object().unwrap(), Some(&overrides));
        assert_eq!(
            params[0].type_hint,
            TypeHint::EntityId {
                entity_name: "todoist_projects".into()
            }
        );
    }

    /// THE PIN: the mapper converts exactly the parameters the registration
    /// rule refuses, across the schema shapes a connector can ship — a name the
    /// rule reads unconditionally (`project_id` as an integer, `status_id` as
    /// an enum), a description it reads only on a `String` (`assignee`), an
    /// array (`ids`), and a value param that must stay untouched. The
    /// comparison runs the shared predicate against the PRE-conversion
    /// hint, which is the hint the rule sees; a rule change that the mapper
    /// does not follow, or a mapper conversion the rule would not have
    /// refused, fails here.
    #[test]
    fn the_mapper_converts_exactly_what_the_rule_refuses() {
        let schema = json!({
            "type": "object",
            "properties": {
                "project_id": {"type": "integer", "description": "The project to file it under"},
                "status_id": {"type": "string", "enum": ["open", "done"]},
                "assignee": {"type": "string", "description": "The ID of the user to assign."},
                "ids": {"type": "array", "items": {"type": "string"}, "description": "The IDs of the tasks."},
                "content": {"type": "string", "description": "The task content"},
            },
            "required": ["project_id", "content"]
        });
        let schema = schema.as_object().unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let params = input_schema_to_params(schema, None);

        for param in &params {
            let prop = properties
                .get(param.name.as_str())
                .unwrap_or_else(|| panic!("no schema for {}", param.name))
                .as_object()
                .expect("a property schema is an object");
            let raw = json_schema_to_type_hint(prop, None);
            let refused = holon_api::id_like_but_undeclared(&param.name, &param.description, &raw);
            assert_eq!(
                param.type_hint == TypeHint::RowKey,
                refused,
                "the rule {} parameter '{}' (schema hint {raw:?}) but the mapper declared {:?}",
                if refused { "refuses" } else { "admits" },
                param.name,
                param.type_hint,
            );
        }

        let hint = |name: &str| {
            params
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("no param {name}"))
                .type_hint
                .clone()
        };
        assert_eq!(hint("ids"), TypeHint::RowKey);
        assert_eq!(hint("content"), TypeHint::String);
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
