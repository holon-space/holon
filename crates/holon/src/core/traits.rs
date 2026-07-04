use std::fmt::Debug;

use holon_api::Value;
// Re-export schema types from holon_api to avoid duplication
pub use holon_api::{DynamicEntity, FieldSchema, IntoEntity, TryFromEntity, TypeDefinition};
use turso;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// NOTE: the enum `Predicate` → SQL path formerly lived here as `trait ToSql` /
// `impl ToSql for Predicate`. It silently returned `None` for `Always` and for
// any And/Or containing a non-compilable child (via `filter_map`), so a valid
// predicate could yield no WHERE clause — a latent data-widening bug (C4
// ruling: "must become disclosed"). It had zero production callers. Replaced by
// `holon_api::computation::Computation::compile_sql` / `predicate_to_sql`,
// which return a typed `Result<SqlFragment, SqlUnsupported>` (fail-loud, never
// silent).

/// Convert a holon_api::Value to turso::Value for database operations.
/// This handles all Value variants including Object and Array by serializing
/// them to JSON.
pub fn value_to_turso(value: &Value) -> turso::Value {
    match value {
        Value::String(s) => turso::Value::Text(s.clone()),
        Value::Integer(i) => turso::Value::Integer(*i),
        Value::Float(f) => turso::Value::Real(*f),
        Value::Boolean(b) => turso::Value::Integer(if *b { 1 } else { 0 }),
        Value::Null => turso::Value::Null,
        // DateTime, Json, Reference, Object, Array all serialize to JSON text
        v => turso::Value::Text(v.to_json_string()),
    }
}

// DELETED (C4 ruling, "generalize the Predicate trait"): the static-dispatch
// `Predicate<T>` trait, its `And`/`Or`/`Not` combinator structs, `Lens<T,U>`,
// `SqlPredicate`, and the `Queryable<T>` trait. This was the old
// generic-over-item query abstraction; its `Queryable::query<P: Predicate<T>>`
// method had no production callers (only a self-test on `QueryableCache`). The
// unified, function-shape-keyed replacement is
// `holon_api::computation::Computation` (in-memory `eval` + disclosed
// `compile_sql`). `QueryableCache` retains its actually-used `query_raw` /
// `query_ordered` / `DataSource` / `EntityCache` surface.

/// Result of an incremental sync operation
#[derive(Debug, Clone)]
pub struct SyncResult<T, Token> {
    /// All items from sync (for full sync) or changed items (for incremental)
    pub items: Vec<T>,
    /// Items that were updated (empty for full sync, populated for incremental)
    pub updated: Vec<T>,
    /// IDs of deleted items (empty for full sync, populated for incremental)
    pub deleted: Vec<String>,
    /// Token for next incremental sync (None if no more updates available)
    pub next_token: Option<Token>,
}

// TypeDefinition, FieldSchema, IntoEntity, TryFromEntity are re-exported from
// holon_api above

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_definition_to_sql() {
        let td = TypeDefinition::new(
            "tasks",
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("title", "TEXT"),
                FieldSchema::new("priority", "INTEGER").indexed().nullable(),
            ],
        );

        let sql = td.to_create_table_sql();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"tasks\""));
        assert!(sql.contains("\"id\" TEXT PRIMARY KEY"));
        assert!(sql.contains("\"title\" TEXT NOT NULL"));
        assert!(sql.contains("\"priority\" INTEGER"));

        let indexes = td.to_index_sql();
        assert_eq!(indexes.len(), 1);
        assert!(indexes[0].contains("CREATE INDEX IF NOT EXISTS idx_tasks_priority"));
    }
}
