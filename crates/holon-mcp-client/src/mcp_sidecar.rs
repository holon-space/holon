use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use holon_api::entity::{FieldSchema, TypeDefinition};
use holon_api::render_types::PreconditionChecker;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpSidecar {
    /// Prefix prepended to all entity names for table names, ID schemes, etc.
    /// E.g. `entity_prefix: "cc_"` turns entity `session` into table `cc_session`
    /// with ID scheme `cc_session:`.
    #[serde(default)]
    pub entity_prefix: Option<String>,
    pub entities: HashMap<String, EntityConfig>,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EntityConfig {
    /// Short display name. Defaults to the entity key name if omitted.
    #[serde(default)]
    pub short_name: Option<String>,
    /// The entity name as reported by the MCP server's resource templates.
    /// Used to match auto-discovered schemas when the YAML key differs from
    /// the server's entity name (e.g. YAML key `cc-session` ↔ server `session`).
    #[serde(default)]
    pub source_name: Option<String>,
    /// Primary key column. Defaults to "id" if omitted.
    #[serde(default)]
    pub id_column: Option<String>,
    /// Schema fields for cache table DDL generation. If present, the entity
    /// can use `QueryableCache::<DynamicEntity>` with a runtime schema.
    #[serde(default)]
    pub schema: Vec<FieldSchema>,
    /// Sync configuration for pulling data from the MCP server (cache table mode).
    pub sync: Option<SyncConfig>,
    /// Virtual table configuration for on-demand querying (foreign table mode).
    /// When set, a Turso foreign table is registered that translates SQL WHERE
    /// constraints into MCP tool parameters.
    #[serde(default)]
    pub vtable: Option<crate::mcp_vtable::VtableConfig>,
    /// Render variants for this entity (presentation layer).
    /// Passed through to `TypeDefinition.profile_variants`.
    #[serde(default)]
    pub profile_variants: Vec<holon_api::ProfileVariant>,
}

impl EntityConfig {
    /// Resolve short_name with fallback to entity key name.
    pub fn short_name_or(&self, entity_name: &str) -> String {
        self.short_name
            .clone()
            .unwrap_or_else(|| entity_name.to_string())
    }

    /// Resolve id_column with fallback to "id".
    pub fn id_column_or_default(&self) -> String {
        self.id_column.clone().unwrap_or_else(|| "id".to_string())
    }
}

/// Declarative sync recipe: either tool-based or resource-based.
///
/// Field presence determines strategy:
/// - `list_tool` present → ToolSync (call tool, extract via `extract_path`)
/// - `list_resource` present → ResourceSync (read resource URI)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    /// MCP tool name to call for listing records (e.g. "list_emails").
    /// Required for tool-based sync.
    pub list_tool: Option<String>,
    /// JSON key in the tool response containing the records array.
    /// Required when `list_tool` is set.
    pub extract_path: Option<String>,
    /// Static parameters passed to the list tool.
    #[serde(default)]
    pub list_params: HashMap<String, serde_json::Value>,
    /// Optional cursor-based incremental sync configuration (tool sync only).
    pub cursor: Option<CursorConfig>,
    /// MCP resource URI (or URI template) to read for listing records.
    /// Required for resource-based sync.
    pub list_resource: Option<String>,
    /// Parameters to expand in the resource URI template (e.g. `{project_id}`).
    #[serde(default)]
    pub uri_params: HashMap<String, String>,
}

impl SyncConfig {
    /// Build a `SyncStrategy` from this config.
    ///
    /// Panics if neither `list_tool` nor `list_resource` is set.
    pub fn into_strategy(&self) -> anyhow::Result<Box<dyn crate::mcp_sync_strategy::SyncStrategy>> {
        use crate::mcp_sync_strategy::{ResourceSync, ToolSync, expand_uri_template};

        if let Some(ref list_tool) = self.list_tool {
            let extract_path = self
                .extract_path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("list_tool requires extract_path"))?;
            Ok(Box::new(ToolSync {
                list_tool: list_tool.clone(),
                extract_path,
                list_params: self.list_params.clone(),
                cursor: self.cursor.clone(),
            }))
        } else if let Some(ref list_resource) = self.list_resource {
            let uri = expand_uri_template(list_resource, &self.uri_params)?;
            Ok(Box::new(ResourceSync { uri }))
        } else {
            anyhow::bail!("SyncConfig must have either list_tool or list_resource");
        }
    }
}

/// Cursor configuration for incremental sync.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CursorConfig {
    /// Field in the tool response containing the new cursor value
    pub response_field: String,
    /// Parameter name to pass the cursor back to the list tool
    pub request_param: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolConfig {
    pub entity: Option<String>,
    pub display_name: Option<String>,
    pub affected_fields: Option<Vec<String>>,
    pub triggered_by: Option<Vec<TriggerConfig>>,
    pub precondition: Option<RhaiPrecondition>,
    pub param_overrides: Option<HashMap<String, ParamOverride>>,
    pub undo: Option<UndoConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UndoConfig {
    Irreversible { reversible: bool },
    Mirror { tool: String, capture: Vec<String> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TriggerConfig {
    pub from: String,
    pub provides: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParamOverride {
    /// e.g. "entity_id:project" → TypeHint::EntityId { entity_name: "todoist_projects" }
    pub type_hint: Option<String>,
}

/// A Rhai expression validated at parse time. Guarantees the expression compiles.
#[derive(Debug, Clone)]
pub struct RhaiPrecondition(String);

impl RhaiPrecondition {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let engine = rhai::Engine::new();
        engine
            .compile_expression(expr)
            .map_err(|e| format!("invalid Rhai precondition '{expr}': {e}"))?;
        Ok(Self(expr.to_string()))
    }

    pub fn to_checker(&self) -> Arc<Box<PreconditionChecker>> {
        let expr = self.0.clone();
        Arc::new(Box::new(move |fields| {
            let engine = rhai::Engine::new();
            let mut scope = rhai::Scope::new();
            for (k, v) in fields {
                if let Some(b) = v.downcast_ref::<bool>() {
                    scope.push(k.clone(), *b);
                } else if let Some(s) = v.downcast_ref::<String>() {
                    scope.push(k.clone(), s.clone());
                } else if let Some(n) = v.downcast_ref::<f64>() {
                    scope.push(k.clone(), *n);
                } else if let Some(n) = v.downcast_ref::<i64>() {
                    scope.push(k.clone(), *n);
                }
            }
            let ast = engine
                .compile_expression(&expr)
                .map_err(|e| format!("Rhai compile error: {e}"))?;
            engine
                .eval_ast::<bool>(&ast)
                .map_err(|e| format!("Rhai eval error for '{expr}': {e}"))
        }))
    }
}

impl Serialize for RhaiPrecondition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RhaiPrecondition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        RhaiPrecondition::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl EntityConfig {
    /// Convert the YAML-declared schema fields into a `TypeDefinition`.
    ///
    /// Sets `graph_label` (PascalCase of table name) and `primary_key` so
    /// `GraphSchemaRegistry` can build GQL schema directly from the result.
    /// Returns `None` if no schema fields are declared.
    pub fn to_type_definition(&self, table_name: &str) -> Option<TypeDefinition> {
        if self.schema.is_empty() {
            return None;
        }
        let mut td = TypeDefinition::new(table_name, self.schema.clone());
        td.graph_label = Some(pascal_case(table_name));
        td.primary_key = self.id_column_or_default();
        td.profile_variants = self.profile_variants.clone();
        Some(td)
    }
}

/// Convert a snake_case or kebab-case name to PascalCase.
fn pascal_case(s: &str) -> String {
    s.split(|c: char| c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.extend(chars);
                    out
                }
            }
        })
        .collect()
}

impl McpSidecar {
    /// Return the primary key column name for an entity (defaults to "id").
    pub fn id_column(&self, entity_name: &str) -> String {
        self.entities
            .get(entity_name)
            .map(|c| c.id_column_or_default())
            .unwrap_or_else(|| "id".to_string())
    }

    /// Return the prefixed `EntityName` for an entity.
    ///
    /// E.g. prefix `"cc_"` + entity `"session"` → `EntityName("cc-session")`.
    /// Use `.as_str()` for URI schemes, `.table_name()` for SQL identifiers.
    pub fn prefixed_name(&self, entity_name: &str) -> holon_api::EntityName {
        let raw = match &self.entity_prefix {
            Some(prefix) => format!("{prefix}{entity_name}"),
            None => entity_name.to_string(),
        };
        holon_api::EntityName::new(raw)
    }

    /// Find a YAML entity key that maps to the given MCP server entity name.
    ///
    /// Checks (in order):
    /// 1. Direct key match (entity key == source name)
    /// 2. Per-entity `source_name` override
    pub fn find_key_by_source_name(&self, source: &str) -> Option<&str> {
        self.entities
            .iter()
            .find(|(_, config)| config.source_name.as_deref() == Some(source))
            .map(|(k, _)| k.as_str())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let sidecar: McpSidecar = serde_yaml::from_str(yaml)?;
        Ok(sidecar)
    }

    pub fn default_entity(&self) -> &str {
        self.entities
            .keys()
            .next()
            .map(|s| s.as_str())
            .expect("sidecar must have at least one entity")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_precondition() {
        let p = RhaiPrecondition::parse("completed == false").unwrap();
        assert_eq!(p.0, "completed == false");
    }

    #[test]
    fn parse_invalid_precondition() {
        let err = RhaiPrecondition::parse("if {{{").unwrap_err();
        assert!(err.contains("invalid Rhai precondition"));
    }

    #[test]
    fn deserialize_sidecar_yaml() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    precondition: "completed == false"
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description]
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sidecar.entities.len(), 1);
        assert_eq!(sidecar.tools.len(), 2);
        assert!(sidecar.tools["complete-tasks"].precondition.is_some());
    }

    #[test]
    fn deserialize_undo_config() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id

tools:
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description]
    undo:
      tool: update-tasks
      capture: [content, description]
  complete-tasks:
    entity: todoist_tasks
    undo:
      reversible: false
  find-tasks:
    entity: todoist_tasks
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();

        let update = &sidecar.tools["update-tasks"];
        match update.undo.as_ref().unwrap() {
            UndoConfig::Mirror { tool, capture } => {
                assert_eq!(tool, "update-tasks");
                assert_eq!(capture, &["content", "description"]);
            }
            other => panic!("expected Mirror, got {:?}", other),
        }

        let complete = &sidecar.tools["complete-tasks"];
        match complete.undo.as_ref().unwrap() {
            UndoConfig::Irreversible { reversible } => assert!(!reversible),
            other => panic!("expected Irreversible, got {:?}", other),
        }

        assert!(sidecar.tools["find-tasks"].undo.is_none());
    }

    #[test]
    fn invalid_precondition_fails_deserialization() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id
tools:
  bad-op:
    precondition: "if {{{"
"#;
        let result: Result<McpSidecar, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_resource_sync_config() {
        let yaml = r#"
entities:
  session:
    short_name: session
    id_column: id
    sync:
      list_resource: "claude-history://projects/{project_id}/sessions"
      uri_params:
        project_id: "-Users-martin-Workspaces-pkm-holon"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["session"].sync.as_ref().unwrap();
        assert!(sync.list_tool.is_none());
        assert_eq!(
            sync.list_resource.as_deref(),
            Some("claude-history://projects/{project_id}/sessions")
        );
        assert_eq!(
            sync.uri_params["project_id"],
            "-Users-martin-Workspaces-pkm-holon"
        );

        let strategy = sync.into_strategy().unwrap();
        assert_eq!(
            strategy.subscribe_uri(),
            Some("claude-history://projects/-Users-martin-Workspaces-pkm-holon/sessions")
        );
    }

    #[test]
    fn deserialize_tool_sync_config() {
        let yaml = r#"
entities:
  task:
    short_name: task
    id_column: id
    sync:
      list_tool: get-tasks
      extract_path: tasks
      list_params:
        project_id: "123"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["task"].sync.as_ref().unwrap();
        assert_eq!(sync.list_tool.as_deref(), Some("get-tasks"));
        assert_eq!(sync.extract_path.as_deref(), Some("tasks"));
        assert!(sync.list_resource.is_none());

        let strategy = sync.into_strategy().unwrap();
        assert!(strategy.subscribe_uri().is_none());
    }

    #[test]
    fn sync_config_neither_tool_nor_resource_fails() {
        let yaml = r#"
entities:
  bad:
    short_name: bad
    id_column: id
    sync:
      list_params: {}
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["bad"].sync.as_ref().unwrap();
        let result = sync.into_strategy();
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("list_tool or list_resource")
        );
    }

    #[test]
    fn minimal_sidecar_entry_defaults() {
        let yaml = r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let entity = &sidecar.entities["session"];
        assert_eq!(entity.short_name_or("session"), "session");
        assert_eq!(entity.id_column_or_default(), "id");
        assert!(entity.schema.is_empty());
        assert!(entity.sync.is_some());
    }

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(super::pascal_case("todoist_task"), "TodoistTask");
        assert_eq!(super::pascal_case("my-entity"), "MyEntity");
        assert_eq!(super::pascal_case("simple"), "Simple");
        assert_eq!(super::pascal_case("a_b_c"), "ABC");
    }

    #[test]
    fn to_type_definition_sets_graph_label_and_primary_key() {
        let yaml = r#"
entities:
  email:
    short_name: mail
    id_column: msg_id
    schema:
      - name: msg_id
        sql_type: TEXT
        primary_key: true
        indexed: true
      - name: subject
        sql_type: TEXT
      - name: read
        sql_type: INTEGER
        nullable: true
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let td = sidecar.entities["email"]
            .to_type_definition("email")
            .expect("should produce TypeDefinition");
        assert_eq!(td.name, "email");
        assert_eq!(td.primary_key, "msg_id");
        assert_eq!(td.graph_label.as_deref(), Some("Email"));
        assert_eq!(td.fields.len(), 3);

        // Verify fields carry through correctly
        assert_eq!(td.fields[0].name, "msg_id");
        assert!(!td.fields[0].nullable);
        assert!(td.fields[2].nullable);
    }
}
