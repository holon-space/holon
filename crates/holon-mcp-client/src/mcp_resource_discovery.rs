use holon_api::entity::FieldSchema;
use rmcp::model::ResourceTemplate;

/// Metadata extracted from a resource template's description for auto-discovery.
#[derive(Debug, Clone)]
pub struct ResourceEntityMeta {
    pub entity_name: String,
    pub primary_keys: Vec<String>,
    pub fields: Vec<FieldSchema>,
    pub uri_template: String,
}

/// Parse entity metadata from a resource template's description.
///
/// Looks for a YAML block separated by `\n---\n` in the description containing:
/// - `entity`: entity name (required)
/// - `primary_keys`: list of primary key columns (required)
/// - `schema`: map of field_name → type_string (required)
///
/// Type mapping: `string→TEXT`, `integer→INTEGER`, `number→REAL`,
/// `boolean→INTEGER`, `object/array→TEXT`.
pub fn parse_resource_template_meta(template: &ResourceTemplate) -> Option<ResourceEntityMeta> {
    let description = template.description.as_ref()?;
    let yaml_block = extract_yaml_block(description)?;

    // ALLOW(ok): optional YAML block may be malformed
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml_block).ok()?;
    let mapping = doc.as_mapping()?;

    let entity_name = mapping
        .get(&serde_yaml::Value::String("entity".into()))?
        .as_str()?
        .to_string();

    let primary_keys: Vec<String> = mapping
        .get(&serde_yaml::Value::String("primary_keys".into()))?
        .as_sequence()?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    if primary_keys.is_empty() {
        return None;
    }

    let schema_value = mapping.get(&serde_yaml::Value::String("schema".into()))?;
    let schema_map = schema_value.as_mapping()?;

    // Support both flat format (`id: string`) and JSON Schema format
    // (`type: object, properties: { id: { type: string } }`)
    let field_map = if schema_map.contains_key(&serde_yaml::Value::String("properties".into())) {
        schema_map
            .get(&serde_yaml::Value::String("properties".into()))?
            .as_mapping()?
    } else {
        schema_map
    };

    let fields: Vec<FieldSchema> = field_map
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str()?;
            // Flat format: value is a string like "string", "integer"
            // JSON Schema format: value is a mapping like { type: "string" }
            let type_str = if let Some(s) = v.as_str() {
                s
            } else if let Some(m) = v.as_mapping() {
                m.get(&serde_yaml::Value::String("type".into()))?.as_str()?
            } else {
                return None;
            };
            let sql_type = map_type_to_sql(type_str);
            let is_pk = primary_keys.contains(&name.to_string());
            let mut field = FieldSchema::new(name, sql_type);
            field.primary_key = is_pk;
            field.nullable = !is_pk;
            Some(field)
        })
        .collect();

    if fields.is_empty() {
        return None;
    }

    Some(ResourceEntityMeta {
        entity_name,
        primary_keys,
        fields,
        uri_template: template.uri_template.clone(),
    })
}

/// Extract the YAML block after `\n---\n` in a description string.
fn extract_yaml_block(description: &str) -> Option<String> {
    let separator = "\n---\n";
    let idx = description.find(separator)?;
    let yaml_part = &description[idx + separator.len()..];
    if yaml_part.trim().is_empty() {
        return None;
    }
    Some(yaml_part.to_string())
}

fn map_type_to_sql(type_str: &str) -> &'static str {
    match type_str {
        "string" => "TEXT",
        "integer" => "INTEGER",
        "number" => "REAL",
        "boolean" => "INTEGER",
        _ => "TEXT",
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::RawResourceTemplate;

    use super::*;

    fn make_template(uri_template: &str, description: &str) -> ResourceTemplate {
        ResourceTemplate {
            raw: RawResourceTemplate {
                uri_template: uri_template.to_string(),
                name: "test".to_string(),
                title: None,
                description: Some(description.to_string()),
                mime_type: None,
            },
            annotations: None,
        }
    }

    #[test]
    fn parse_valid_resource_template_meta() {
        let desc = "List of projects\n---\nentity: project\nprimary_keys: [id]\nschema:\n  id: string\n  name: string\n  path: string";
        let template = make_template("claude-history://projects", desc);
        let meta = parse_resource_template_meta(&template).unwrap();

        assert_eq!(meta.entity_name, "project");
        assert_eq!(meta.primary_keys, vec!["id"]);
        assert_eq!(meta.fields.len(), 3);
        assert_eq!(meta.fields[0].name, "id");
        assert_eq!(meta.fields[0].sql_type, "TEXT");
        assert!(meta.fields[0].primary_key);
        assert!(!meta.fields[0].nullable);
    }

    #[test]
    fn parse_with_various_types() {
        let desc = "Sessions\n---\nentity: session\nprimary_keys: [id]\nschema:\n  id: string\n  count: integer\n  score: number\n  active: boolean\n  meta: object";
        let template = make_template("test://sessions", desc);
        let meta = parse_resource_template_meta(&template).unwrap();

        let types: Vec<&str> = meta.fields.iter().map(|f| f.sql_type.as_str()).collect();
        assert!(types.contains(&"TEXT"));
        assert!(types.contains(&"INTEGER"));
        assert!(types.contains(&"REAL"));
    }

    #[test]
    fn no_yaml_block_returns_none() {
        let template = make_template("test://x", "Just a plain description");
        assert!(parse_resource_template_meta(&template).is_none());
    }

    #[test]
    fn missing_entity_returns_none() {
        let desc = "Test\n---\nprimary_keys: [id]\nschema:\n  id: string";
        let template = make_template("test://x", desc);
        assert!(parse_resource_template_meta(&template).is_none());
    }

    #[test]
    fn parse_json_schema_format() {
        let desc = "Project Sessions\n---\nentity: session\nprimary_keys: [id]\nschema:\n  type: object\n  properties:\n    id:\n      type: string\n    project_id:\n      type: string\n    message_count:\n      type: integer\n    is_sidechain:\n      type: integer";
        let template = make_template("claude-history://projects/{project_id}/sessions", desc);
        let meta = parse_resource_template_meta(&template).unwrap();

        assert_eq!(meta.entity_name, "session");
        assert_eq!(meta.primary_keys, vec!["id"]);
        assert_eq!(meta.fields.len(), 4);

        let id_field = meta.fields.iter().find(|f| f.name == "id").unwrap();
        assert!(id_field.primary_key);
        assert_eq!(id_field.sql_type, "TEXT");

        let count_field = meta
            .fields
            .iter()
            .find(|f| f.name == "message_count")
            .unwrap();
        assert!(!count_field.primary_key);
        assert_eq!(count_field.sql_type, "INTEGER");
    }

    #[test]
    fn no_description_returns_none() {
        let template = ResourceTemplate {
            raw: RawResourceTemplate {
                uri_template: "test://x".to_string(),
                name: "test".to_string(),
                title: None,
                description: None,
                mime_type: None,
            },
            annotations: None,
        };
        assert!(parse_resource_template_meta(&template).is_none());
    }
}
