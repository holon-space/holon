//! Entity types and traits for the Entity derive macro.
//!
//! Core types:
//! - `TypeDefinition`: The canonical entity schema (DDL, GQL, field lifetimes)
//! - `FieldSchema`, `FieldLifetime`: Per-field definition with storage lifetime
//! - `DynamicEntity`: Type-erased runtime entity representation
//! - `IntoEntity`, `TryFromEntity`: Traits for entity conversion

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub fields: HashMap<String, Value>,
}

impl DynamicEntity {
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: HashMap::new(),
        }
    }

    pub fn with_field(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<Value>) {
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

/// Determines where a field's data lives and how it survives cache reconstruction.
///
/// | Lifetime     | Loro | Org/YAML | Turso | CRDT merge | Reconstruction        |
/// |--------------|------|----------|-------|------------|-----------------------|
/// | `Persistent` | Yes  | Yes      | Yes   | Yes        | From Loro             |
/// | `Computed`   | No   | No       | Yes   | No         | Recompute from expr   |
/// | `Transient`  | No   | No       | Yes   | No         | Re-fetch from source  |
/// | `Historical` | No   | No       | Yes+backup | No   | From backup           |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FieldLifetime {
    Persistent,
    Computed {
        expr: holon_engine::guard::CompiledExpr,
    },
    Transient,
    Historical,
}

impl Default for FieldLifetime {
    fn default() -> Self {
        Self::Persistent
    }
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TypeSource {
    /// Hard-coded in Rust (Block).
    BuiltIn,
    /// Ships with the app but user can extend (Person, Organization).
    PreConfigured,
    /// Created by the user at runtime via YAML.
    UserDefined,
    /// From an MCP sidecar configuration.
    McpProvider(String),
}

impl Default for TypeSource {
    fn default() -> Self {
        Self::UserDefined
    }
}

/// A render variant for an entity type (presentation layer).
///
/// Variants are checked in priority order (highest first). The first variant
/// whose condition matches (or has no condition) is used to render the entity.
/// Conditions are `CompiledExpr` — pre-compiled at the YAML deserialization boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileVariant {
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    /// Rhai condition expression. Compiled at deserialization time.
    /// None = unconditional (always matches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<holon_engine::guard::CompiledExpr>,
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
    /// GQL graph label (e.g. "Block", "Person"). None = not exposed as GQL node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_label: Option<String>,
    /// Where this definition came from.
    #[serde(default)]
    pub source: TypeSource,
    /// Render variants for this entity type (presentation layer).
    /// Each variant defines a named render expression with optional Rhai condition.
    /// Conditions are pre-compiled at the serde boundary (CompiledExpr custom deserialize).
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
        let columns: Vec<String> = self
            .fields
            .iter()
            .map(|f| {
                let mut col = format!("{} {}", f.name, f.sql_type);
                if f.primary_key {
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

        format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" (\n  {}\n)",
            self.name,
            columns.join(",\n  ")
        )
    }

    /// Generate `CREATE INDEX` statements for indexed non-PK fields.
    pub fn to_index_sql(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.indexed && !f.primary_key)
            .map(|f| {
                format!(
                    "CREATE INDEX IF NOT EXISTS idx_{}_{} ON \"{}\" ({})",
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

    /// Fields with `Computed` lifetime, returned as `(name, CompiledExpr)` pairs.
    pub fn computed_fields(&self) -> Vec<(&str, &holon_engine::guard::CompiledExpr)> {
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
                    scope.push(key.clone(), s.clone());
                }
                Value::Integer(i) => {
                    scope.push(key.clone(), *i);
                }
                Value::Float(f) => {
                    scope.push(key.clone(), *f);
                }
                Value::Boolean(b) => {
                    scope.push(key.clone(), *b);
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
                    row.insert(field.name.clone(), value);
                }
                Err(e) => {
                    tracing::debug!(
                        "Computed field '{}' on '{}' failed: {e}",
                        field.name,
                        self.name
                    );
                    row.insert(field.name.clone(), Value::Null);
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

/// Parse a dynamic entity back into a typed entity. Can fail if fields are missing/wrong type.
/// flutter_rust_bridge:ignore
pub trait TryFromEntity: Sized {
    fn from_entity(entity: DynamicEntity) -> Result<Self>;
}

// Identity conversions for DynamicEntity — used by QueryableCache<DynamicEntity>
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

/// Type alias for entity storage as HashMap
pub type StorageEntity = HashMap<String, Value>;

// =============================================================================
// Graph schema intermediate types
// =============================================================================

/// Graph node definition for non-Entity tables/views (e.g., materialized views).
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
