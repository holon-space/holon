//! Entity types and traits for the Entity derive macro.
//!
//! Core types:
//! - `TypeDefinition`: The canonical entity schema (DDL, GQL, field lifetimes)
//! - `FieldSchema`, `FieldLifetime`: Per-field definition with storage lifetime
//! - `DynamicEntity`: Type-erased runtime entity representation
//! - `IntoEntity`, `TryFromEntity`: Traits for entity conversion

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use crate::Value;

/// Result type for entity operations
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// =============================================================================
// DynamicEntity - Runtime entity representation
// =============================================================================

/// A dynamic entity with runtime-determined fields.
///
/// This provides a type-erased representation of any entity,
/// useful for generic storage and serialization.
///
/// flutter_rust_bridge:ignore
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicEntity {
    pub type_name: String,
    pub fields: StorageEntity,
}

impl DynamicEntity {
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: StorageEntity::new(),
        }
    }

    pub fn with_field(
        mut self,
        name: impl Into<std::sync::Arc<str>>,
        value: impl Into<Value>,
    ) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    pub fn set(&mut self, name: impl Into<std::sync::Arc<str>>, value: impl Into<Value>) {
        self.fields.insert(name.into(), value.into());
    }

    /// flutter_rust_bridge:ignore
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    /// flutter_rust_bridge:ignore
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.fields.get_mut(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.fields.remove(name)
    }

    pub fn has_field(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        self.get(name).and_then(|v| v.as_string().map(String::from))
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.as_i64())
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(|v| v.as_bool())
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|v| v.as_f64())
    }
}

impl Default for DynamicEntity {
    fn default() -> Self {
        Self::new("unknown")
    }
}

// Schema struct removed — replaced by TypeDefinition.

// =============================================================================
// Field lifetime — governs where a field's data is stored and how it's
// reconstructed after a cache wipe.
// =============================================================================

/// Determines where a field's data lives and how it survives cache
/// reconstruction.
///
/// | Lifetime     | Loro | Org/YAML | Turso | CRDT merge | Reconstruction        |
/// |--------------|------|----------|-------|------------|-----------------------|
/// | `Persistent` | Yes  | Yes      | Yes   | Yes        | From Loro             |
/// | `Computed`   | No   | No       | Yes   | No         | Recompute from expr   |
/// | `Transient`  | No   | No       | Yes   | No         | Re-fetch from source  |
/// | `Historical` | No   | No       | Yes+backup | No   | From backup           |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldLifetime {
    #[default]
    Persistent,
    Computed {
        expr: holon_expr::CompiledExpr,
    },
    Transient,
    Historical,
}

// =============================================================================
// FieldSchema — single field definition
// =============================================================================

/// Schema for a single field in a table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FieldSchema {
    pub name: String,
    pub sql_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub indexed: bool,
    #[serde(rename = "jsonb")]
    pub is_jsonb: bool,
    /// SQL DEFAULT expression (e.g., `"0"`, `"'text'"`, `"(datetime('now'))"`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    /// Where this field's data lives. Defaults to `Persistent`.
    #[serde(default)]
    pub lifetime: FieldLifetime,
    /// GQL edge name for reference fields (e.g., `"CHILD_OF"`).
    /// Set by `#[reference(entity = "...", edge = "...")]` on Entity structs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edge_name: Option<String>,
    /// Target entity name for reference/FK fields (e.g., `"block"`).
    /// Set by `#[reference(entity = "...")]` on Entity structs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reference_target: Option<String>,
}

impl Default for FieldSchema {
    fn default() -> Self {
        Self {
            name: String::new(),
            sql_type: "TEXT".to_string(),
            nullable: false,
            primary_key: false,
            indexed: false,
            is_jsonb: false,
            default_value: None,
            lifetime: FieldLifetime::default(),
            edge_name: None,
            reference_target: None,
        }
    }
}

impl FieldSchema {
    pub fn new(name: impl Into<String>, sql_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sql_type: sql_type.into(),
            ..Default::default()
        }
    }

    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    pub fn primary_key(mut self) -> Self {
        self.primary_key = true;
        self
    }

    pub fn indexed(mut self) -> Self {
        self.indexed = true;
        self
    }

    pub fn jsonb(mut self) -> Self {
        self.is_jsonb = true;
        self
    }

    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn lifetime(mut self, lifetime: FieldLifetime) -> Self {
        self.lifetime = lifetime;
        self
    }

    pub fn edge_name(mut self, name: impl Into<String>) -> Self {
        self.edge_name = Some(name.into());
        self
    }

    pub fn reference_target(mut self, target: impl Into<String>) -> Self {
        self.reference_target = Some(target.into());
        self
    }
}

// =============================================================================
// TypeDefinition — the canonical entity schema type
// =============================================================================

/// Where this type definition originated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TypeSource {
    /// Hard-coded in Rust (Block).
    BuiltIn,
    /// Ships with the app but user can extend (Person, Organization).
    PreConfigured,
    /// Created by the user at runtime via YAML.
    #[default]
    UserDefined,
    /// From an MCP sidecar configuration.
    McpProvider(String),
}

/// A render variant for an entity type (presentation layer).
///
/// Variants are checked in priority order (highest first). The first variant
/// whose condition matches (or has no condition) is used to render the entity.
/// Conditions are `CompiledExpr` — pre-compiled at the YAML deserialization
/// boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileVariant {
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    /// Rhai condition expression. Compiled at deserialization time.
    /// None = unconditional (always matches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<holon_expr::CompiledExpr>,
    /// Render DSL expression string (parsed by render_dsl at resolution time).
    pub render: String,
}

/// The canonical entity schema. Every entity in Holon — whether hard-coded
/// (Block), pre-configured (Person), user-defined, or MCP-sourced — is
/// represented by a `TypeDefinition`.
///
/// Replaces the former `Schema` struct. Provides DDL generation, GQL
/// registration metadata, and field lifetime awareness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDefinition {
    /// Entity/table name (e.g. "block", "person", "todoist_task").
    pub name: String,
    /// Default lifetime for fields that don't declare one explicitly.
    #[serde(default)]
    pub default_lifetime: FieldLifetime,
    /// Field definitions.
    pub fields: Vec<FieldSchema>,
    /// Primary key column name. Defaults to "id".
    #[serde(default = "default_primary_key")]
    pub primary_key: String,
    /// If set, the PK column gets a `REFERENCES {table}(id)` FK constraint.
    /// Used for extension tables that reference the `block` table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_references: Option<String>,
    /// GQL graph label (e.g. "Block", "Person"). None = not exposed as GQL
    /// node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_label: Option<String>,
    /// Where this definition came from.
    #[serde(default)]
    pub source: TypeSource,
    /// Render variants for this entity type (presentation layer).
    /// Each variant defines a named render expression with optional Rhai
    /// condition. Conditions are pre-compiled at the serde boundary
    /// (CompiledExpr custom deserialize).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_variants: Vec<ProfileVariant>,
}

fn default_primary_key() -> String {
    "id".to_string()
}

impl TypeDefinition {
    pub fn new(name: impl Into<String>, fields: Vec<FieldSchema>) -> Self {
        Self {
            name: name.into(),
            default_lifetime: FieldLifetime::default(),
            fields,
            primary_key: "id".to_string(),
            id_references: None,
            graph_label: None,
            source: TypeSource::default(),
            profile_variants: Vec::new(),
        }
    }

    /// Generate `CREATE TABLE IF NOT EXISTS` SQL.
    pub fn to_create_table_sql(&self) -> String {
        assert!(
            !self.fields.is_empty(),
            "Cannot generate CREATE TABLE for type '{}' with no fields.",
            self.name
        );
        // SQLite forbids multiple inline `PRIMARY KEY` annotations — when 2+
        // fields are flagged we emit a table-level `PRIMARY KEY (a, b, …)`
        // clause and skip the inline form. The single-PK case keeps inline so
        // `REFERENCES` (for id-link tables) still attaches to the right column.
        let pk_count = self.fields.iter().filter(|f| f.primary_key).count();
        let inline_pk = pk_count == 1;

        let columns: Vec<String> = self
            .fields
            .iter()
            .map(|f| {
                let mut col = format!("\"{}\" {}", f.name, f.sql_type);
                if f.primary_key && inline_pk {
                    col.push_str(" PRIMARY KEY");
                    if let Some(ref target) = self.id_references {
                        col.push_str(&format!(" REFERENCES \"{target}\"(id)"));
                    }
                }
                if !f.nullable {
                    col.push_str(" NOT NULL");
                }
                if let Some(ref default) = f.default_value {
                    col.push_str(" DEFAULT ");
                    col.push_str(default);
                }
                col
            })
            .collect();

        let mut body = columns.join(",\n  ");
        if pk_count >= 2 {
            let pk_cols: Vec<&str> = self
                .fields
                .iter()
                .filter(|f| f.primary_key)
                .map(|f| f.name.as_str())
                .collect();
            let quoted_pk: Vec<String> = pk_cols.iter().map(|c| format!("\"{c}\"")).collect();
            body.push_str(&format!(",\n  PRIMARY KEY ({})", quoted_pk.join(", ")));
        }

        format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" (\n  {}\n)",
            self.name, body
        )
    }

    /// Generate `CREATE INDEX` statements for indexed non-PK fields.
    pub fn to_index_sql(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.indexed && !f.primary_key)
            .map(|f| {
                format!(
                    "CREATE INDEX IF NOT EXISTS idx_{}_{} ON \"{}\" (\"{}\")",
                    self.name, f.name, self.name, f.name
                )
            })
            .collect()
    }

    /// Check if a field is marked as JSONB.
    pub fn field_is_jsonb(&self, field_name: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.name == field_name)
            .map(|f| f.is_jsonb)
            .unwrap_or(false)
    }

    /// Create a minimal `TypeDefinition` from just a table name (no fields).
    /// Only for query/insert contexts — cannot generate DDL.
    pub fn from_table_name(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default_lifetime: FieldLifetime::default(),
            fields: Vec::new(),
            primary_key: "id".to_string(),
            id_references: None,
            graph_label: None,
            profile_variants: Vec::new(),
            source: TypeSource::default(),
        }
    }

    /// Fields with `Persistent` lifetime (stored in Loro + org/YAML + Turso).
    pub fn persistent_fields(&self) -> Vec<&FieldSchema> {
        self.fields
            .iter()
            .filter(|f| matches!(f.lifetime, FieldLifetime::Persistent))
            .collect()
    }

    /// Fields with `Computed` lifetime, returned as `(name, CompiledExpr)`
    /// pairs.
    pub fn computed_fields(&self) -> Vec<(&str, &holon_expr::CompiledExpr)> {
        self.fields
            .iter()
            .filter_map(|f| match &f.lifetime {
                FieldLifetime::Computed { expr } => Some((f.name.as_str(), expr)),
                _ => None,
            })
            .collect()
    }

    /// Fields with `Transient` lifetime (Turso only, device-local).
    pub fn transient_fields(&self) -> Vec<&FieldSchema> {
        self.fields
            .iter()
            .filter(|f| matches!(f.lifetime, FieldLifetime::Transient))
            .collect()
    }

    /// Evaluate computed fields and merge results into the row.
    ///
    /// Uses a default Rhai engine. For expressions that need custom functions
    /// (e.g., entity lookups), use `enrich_with()` instead.
    pub fn enrich(&self, row: StorageEntity) -> StorageEntity {
        let engine = rhai::Engine::new();
        self.enrich_with(row, &engine)
    }

    /// Evaluate computed fields with a caller-provided Rhai engine.
    ///
    /// Allows callers to register custom functions (e.g., `document()`,
    /// `query_source()` backed by LiveEntities) before evaluation.
    /// Fields must be in topological order (dependencies before dependents) —
    /// `TypeRegistry::register()` ensures this via topo-sort.
    pub fn enrich_with(&self, mut row: StorageEntity, engine: &rhai::Engine) -> StorageEntity {
        let mut scope = rhai::Scope::new();

        for (key, value) in &row {
            match value {
                Value::String(s) => {
                    scope.push(key.as_ref(), s.clone());
                }
                Value::Integer(i) => {
                    scope.push(key.as_ref(), *i);
                }
                Value::Float(f) => {
                    scope.push(key.as_ref(), *f);
                }
                Value::Boolean(b) => {
                    scope.push(key.as_ref(), *b);
                }
                _ => {}
            }
        }

        for field in &self.fields {
            let FieldLifetime::Computed { expr } = &field.lifetime else {
                continue;
            };
            match engine.eval_ast_with_scope::<rhai::Dynamic>(&mut scope, &expr.ast) {
                Ok(result) => {
                    let value = dynamic_to_value(result.clone());
                    scope.push(field.name.clone(), result);
                    row.insert(field.name.as_str().into(), value);
                }
                Err(e) => {
                    tracing::debug!(
                        "Computed field '{}' on '{}' failed: {e}",
                        field.name,
                        self.name
                    );
                    row.insert(field.name.as_str().into(), Value::Null);
                }
            }
        }
        row
    }
}

/// Convert a Rhai Dynamic value to a holon Value.
fn dynamic_to_value(d: rhai::Dynamic) -> Value {
    if let Ok(s) = d.clone().into_string() {
        Value::String(s)
    } else if let Ok(i) = d.as_int() {
        Value::Integer(i)
    } else if let Ok(f) = d.as_float() {
        Value::Float(f)
    } else if let Ok(b) = d.as_bool() {
        Value::Boolean(b)
    } else {
        Value::Null
    }
}

// =============================================================================
// Entity conversion traits
// =============================================================================

/// Convert a typed entity to its dynamic (HashMap) representation.
/// flutter_rust_bridge:ignore
pub trait IntoEntity {
    fn to_entity(&self) -> DynamicEntity;
    fn type_definition() -> TypeDefinition;
}

/// Parse a dynamic entity back into a typed entity. Can fail if fields are
/// missing/wrong type. flutter_rust_bridge:ignore
pub trait TryFromEntity: Sized {
    fn from_entity(entity: DynamicEntity) -> Result<Self>;
}

// Identity conversions for DynamicEntity — used by
// QueryableCache<DynamicEntity>
impl IntoEntity for DynamicEntity {
    fn to_entity(&self) -> DynamicEntity {
        self.clone()
    }

    fn type_definition() -> TypeDefinition {
        TypeDefinition::new("dynamic_entity", vec![])
    }
}

impl TryFromEntity for DynamicEntity {
    fn from_entity(entity: DynamicEntity) -> Result<Self> {
        Ok(entity)
    }
}

// =============================================================================
// StorageEntity type alias
// =============================================================================

/// Type alias for entity storage as HashMap.
/// Keys are `Arc<str>` so identical column names are shared across rows
/// instead of allocating a fresh String per cell.
pub type StorageEntity = HashMap<std::sync::Arc<str>, Value>;

// =============================================================================
// StorageEntity operation-param keys (the write-path param contract)
// =============================================================================
//
// These name operation-control entries that producers add to the params
// `StorageEntity` they hand to a create/update/delete op. They are part of the
// shared kernel (this is where `StorageEntity` itself lives), not specific to
// any backend — `SqlOperationProvider` reads them and keeps them out of the
// persisted row. `holon-loro`'s `event_bus` re-exports these for back-compat.

/// Param-side name for the document-routing hint — names the document URI that
/// owns the block this op applies to. Producers (`build_block_params` in
/// `holon-orgmode`, plus internal lookups in `SqlOperationProvider`) add this
/// to the params HashMap they hand to a create/update/delete op. The leading
/// underscore lets `SqlOperationProvider::partition_params` recognise it as
/// operation-control metadata (via the `_routing_` prefix) and keep it out of
/// the persisted row.
// ALLOW(routing_payload_key): producer-side param const, see doc-comment above.
pub const ROUTING_DOC_URI_KEY: &str = "_routing_doc_uri";

/// Param-side name for the typed positional intent — names the predecessor
/// sibling a freshly-created block should sit after. Producers
/// (`BlockOperations::split_block`, `FileSyncController::on_file_changed`) add
/// this key to the params HashMap they hand to a create op;
/// `SqlOperationProvider` reads it (to drive sibling ordering) and drops it
/// from the persisted row.
pub const POSITION_AFTER_BLOCK_ID_PARAM: &str = "after_block_id";

/// Operation-control param carrying a minted position's sibling re-keys to the
/// write that consumes the key, so both land in ONE transaction (ADR 0030 D1).
/// Never persisted: the SQL writer lifts it into statements and drops it.
///
/// INTERNAL. It travels in the same params map an outside caller can populate,
/// so a writer acting on it MUST prove its targets rather than trust them, and
/// every intent boundary MUST refuse it — see [`is_operation_control_param`].
pub const ORDER_REKEYS_PARAM: &str = "_order_rekeys";

/// True for the params keys a writer INTERPRETS rather than stores: positional
/// intent, sibling re-keys, routing hints, diff guards.
///
/// This is the operation-control namespace. It is the set the SQL provider's
/// `partition_params` refuses to PERSIST, AND the set every intent boundary
/// (`BlockWriteField::parse`, the MCP tool boundary, the Loro→SQL projection)
/// must refuse to ACCEPT — these keys are instructions, so taking one from
/// outside hands the caller a writer primitive the boundary otherwise refuses
/// (ADR 0005 keeps the order key out of `BlockWriteField` for the same reason).
/// One definition so the persist-side strip and the accept-side refusal cannot
/// drift.
///
/// Underscore-prefixed keys are NOT all control: `_source_header_args` and
/// `_source_results` are real block properties and must keep flowing through,
/// which is why this is a named set and not a prefix test.
pub fn is_operation_control_param(key: &str) -> bool {
    key == ORDER_REKEYS_PARAM
        || key == POSITION_AFTER_BLOCK_ID_PARAM
        || key.starts_with("_routing_")
        || key.starts_with("_expected_")
}

// =============================================================================
// Graph schema intermediate types
// =============================================================================

/// Graph node definition for non-Entity tables/views (e.g., materialized
/// views).
///
/// Used by `SchemaModule::graph_contributions()` to register GQL nodes
/// for database objects that don't have a corresponding Rust Entity struct.
#[derive(Debug, Clone)]
pub struct GraphNodeDef {
    /// GQL node label (e.g., "FocusRoot")
    pub label: String,
    /// Underlying SQL table or view name
    pub table_name: String,
    /// Primary key / id column name
    pub id_column: String,
    /// Column mappings: (gql_property_name, sql_column_name)
    pub columns: Vec<(String, String)>,
}

/// Graph edge definition for non-Entity relationships.
///
/// Used by `SchemaModule::graph_contributions()` to register GQL edges
/// that aren't derivable from Entity `#[reference]` annotations.
#[derive(Debug, Clone)]
pub struct GraphEdgeDef {
    /// GQL edge type name (e.g., "FOCUSES_ON")
    pub edge_name: String,
    /// Source node label constraint (None = any)
    pub source_label: Option<String>,
    /// Target node label constraint (None = any)
    pub target_label: Option<String>,
    /// Table containing the foreign key
    pub fk_table: String,
    /// Foreign key column name
    pub fk_column: String,
    /// Target table name
    pub target_table: String,
    /// Target table's ID column
    pub target_id_column: String,
}

#[cfg(test)]
mod create_table_sql_tests {
    use super::*;

    #[test]
    fn composite_primary_key_emits_table_level_clause() {
        let td = TypeDefinition {
            name: "gh_issue".to_string(),
            graph_label: None,
            primary_key: "owner".to_string(),
            fields: vec![
                FieldSchema::new("owner", "TEXT").primary_key(),
                FieldSchema::new("repo", "TEXT").primary_key(),
                FieldSchema::new("number", "INTEGER").primary_key(),
                FieldSchema::new("title", "TEXT"),
            ],
            id_references: None,
            profile_variants: vec![],
            default_lifetime: FieldLifetime::default(),
            source: TypeSource::default(),
        };
        let sql = td.to_create_table_sql();
        // Inline PRIMARY KEY annotations would have made SQLite reject this DDL
        // with "table has more than one primary key" — the table-level clause
        // is the only legal form.
        assert!(
            !sql.contains("\"owner\" TEXT PRIMARY KEY"),
            "inline PK leaked: {sql}"
        );
        assert!(
            sql.contains("PRIMARY KEY (\"owner\", \"repo\", \"number\")"),
            "expected composite PK clause with quoted identifiers; got: {sql}"
        );
    }

    #[test]
    fn single_primary_key_keeps_inline_form() {
        let td = TypeDefinition {
            name: "single".to_string(),
            graph_label: None,
            primary_key: "id".to_string(),
            fields: vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("name", "TEXT"),
            ],
            id_references: None,
            profile_variants: vec![],
            default_lifetime: FieldLifetime::default(),
            source: TypeSource::default(),
        };
        let sql = td.to_create_table_sql();
        assert!(sql.contains("\"id\" TEXT PRIMARY KEY"), "got: {sql}");
        assert!(!sql.contains("PRIMARY KEY (\"id\")"), "got: {sql}");
    }
}

#[cfg(test)]
mod mutation_gap_tests {
    use super::*;

    #[test]
    fn field_schema_builder_chain_preserves_everything() {
        let f = FieldSchema::new("score", "INTEGER")
            .nullable()
            .indexed()
            .default_value("0")
            .lifetime(FieldLifetime::Transient)
            .edge_name("SCORED_BY")
            .reference_target("block");

        assert_eq!(f.name, "score");
        assert_eq!(f.sql_type, "INTEGER");
        assert!(f.nullable);
        assert!(f.indexed);
        assert!(!f.primary_key);
        assert_eq!(f.default_value.as_deref(), Some("0"));
        assert!(matches!(f.lifetime, FieldLifetime::Transient));
        assert_eq!(f.edge_name.as_deref(), Some("SCORED_BY"));
        assert_eq!(f.reference_target.as_deref(), Some("block"));
    }

    #[test]
    fn type_definition_lifetime_projections_and_index_sql() {
        let engine = rhai::Engine::new();
        let expr = holon_expr::CompiledExpr::compile(&engine, "a + 1").unwrap();

        let td = TypeDefinition::new(
            "thing",
            vec![
                FieldSchema::new("id", "TEXT").primary_key().indexed(),
                FieldSchema::new("title", "TEXT").indexed(),
                FieldSchema::new("score", "INTEGER").lifetime(FieldLifetime::Computed { expr }),
                FieldSchema::new("cache", "TEXT")
                    .indexed()
                    .lifetime(FieldLifetime::Transient),
            ],
        );

        let persistent: Vec<&str> = td
            .persistent_fields()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(persistent, vec!["id", "title"]);

        let computed: Vec<&str> = td.computed_fields().iter().map(|(n, _)| *n).collect();
        assert_eq!(computed, vec!["score"]);

        let transient: Vec<&str> = td
            .transient_fields()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(transient, vec!["cache"]);

        // Indexed non-PK fields only: no idx for the primary key.
        let idx = td.to_index_sql();
        assert_eq!(
            idx,
            vec![
                "CREATE INDEX IF NOT EXISTS idx_thing_title ON \"thing\" (\"title\")".to_string(),
                "CREATE INDEX IF NOT EXISTS idx_thing_cache ON \"thing\" (\"cache\")".to_string(),
            ]
        );

        // enrich evaluates the computed field from row scope.
        let row: StorageEntity = [(std::sync::Arc::<str>::from("a"), Value::Integer(2))]
            .into_iter()
            .collect();
        let enriched = td.enrich(row);
        assert_eq!(enriched.get("score"), Some(&Value::Integer(3)));
        assert_eq!(enriched.get("a"), Some(&Value::Integer(2)));
    }

    #[test]
    fn primary_key_defaults_to_id_on_deserialize() {
        let td: TypeDefinition = serde_json::from_str(r#"{"name":"t","fields":[]}"#).unwrap();
        assert_eq!(td.primary_key, "id");
    }

    #[test]
    fn dynamic_entity_typed_getters() {
        let mut e = DynamicEntity::new("block")
            .with_field("s", "str")
            .with_field("i", 42i64)
            .with_field("b", true)
            .with_field("f", 1.5f64);

        assert_eq!(e.type_name, "block");
        assert!(e.has_field("s"));
        assert!(!e.has_field("nope"));
        assert_eq!(e.get_string("s"), Some("str".to_string()));
        assert_eq!(e.get_i64("i"), Some(42));
        assert_eq!(e.get_bool("b"), Some(true));
        assert_eq!(e.get_f64("f"), Some(1.5));
        assert_eq!(e.get_string("i"), None);
        assert_eq!(e.get_i64("missing"), None);

        e.set("s", "new");
        assert_eq!(e.get_string("s"), Some("new".to_string()));
        *e.get_mut("i").unwrap() = Value::Integer(7);
        assert_eq!(e.get_i64("i"), Some(7));
        assert_eq!(e.remove("b"), Some(Value::Boolean(true)));
        assert!(!e.has_field("b"));
        assert_eq!(e.remove("b"), None);
    }
}
