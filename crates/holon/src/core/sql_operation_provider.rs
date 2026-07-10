//! SQL-based operation provider for blocks
//!
//! Provides direct SQL access to block operations, bypassing the Loro CRDT layer.
//! Used when OrgMode is enabled but Loro is disabled, or by any component that
//! needs to write blocks directly to the database.

use std::collections::HashMap;
use std::collections::HashSet;

use async_trait::async_trait;

use crate::storage::schema_module::EdgeFieldDescriptor;
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::DbHandle;
use crate::sync::event_bus::{EventOrigin, POSITION_AFTER_BLOCK_ID_PARAM};
use holon_api::{
    EntityName, EntityUri, OperationDescriptor, OperationParam, ParentNotFound, TypeHint, Value,
};
use holon_core::storage::types::StorageEntity;
use holon_core::{OperationProvider, OperationResult, OriginTaggedWrites, Result};

pub(crate) fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Integer(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Null => serde_json::Value::Null,
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => serde_json::from_str(s).unwrap_or_else(|e| {
            panic!(
                "[value_to_json] Value::Json contains invalid JSON {:?}: {}",
                s, e
            )
        }),
        Value::Array(arr) => serde_json::Value::Array(arr.iter().map(value_to_json).collect()),
        Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect(),
        ),
    }
}

/// Serialize a property map to canonical JSON (keys sorted by BTreeMap).
///
/// All callers writing the `properties` column must go through this so the
/// diff guard's string comparison in `prepare_update` is stable regardless of
/// insertion order.
fn properties_to_canonical_json<I>(props: I) -> String
where
    I: IntoIterator<Item = (String, serde_json::Value)>,
{
    let canonical: std::collections::BTreeMap<_, _> = props.into_iter().collect();
    serde_json::to_string(&canonical).expect("properties must serialize")
}

/// Compare a SQL literal string (as produced by `value_to_sql`) with a stored
/// `Value` from the DB. Returns `true` when they represent the same value.
///
/// Used by `prepare_update`'s Rust diff guard to drop pairs that haven't changed
/// without relying on Turso's `IS NOT` string-comparison semantics.
fn sql_literal_equals_value(sql_literal: &str, db_val: Option<&Value>) -> bool {
    // NULL on either side.
    if sql_literal == "NULL" {
        return matches!(db_val, None | Some(Value::Null));
    }
    let Some(db) = db_val else {
        return false; // new value is non-NULL, old is missing → changed
    };
    // Quoted string literal: `'...'` with internal `''` escapes.
    if let Some(inner) = sql_literal
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    {
        let unescaped = inner.replace("''", "'");
        return match db {
            Value::String(s) => {
                if *s == unescaped {
                    return true;
                }
                // Try semantic JSON comparison: `{"b":1,"a":2}` == `{"a":2,"b":1}`.
                // This handles the `properties` column where the stored value may be
                // non-canonical (insertion-ordered) while new values are always
                // BTreeMap-sorted by `properties_to_canonical_json`.
                // We canonicalize both via BTreeMap round-trip because serde_json
                // may use `preserve_order` (IndexMap), making Value equality
                // order-sensitive.
                if let (Ok(a), Ok(b)) = (
                    serde_json::from_str::<serde_json::Value>(&unescaped),
                    serde_json::from_str::<serde_json::Value>(s.as_str()),
                ) {
                    fn canonical_json(v: serde_json::Value) -> String {
                        match v {
                            serde_json::Value::Object(map) => {
                                let sorted: std::collections::BTreeMap<_, _> =
                                    map.into_iter().collect();
                                serde_json::to_string(&sorted).unwrap_or_default()
                            }
                            other => serde_json::to_string(&other).unwrap_or_default(),
                        }
                    }
                    return canonical_json(a) == canonical_json(b);
                }
                false
            }
            Value::Null => false,
            Value::Object(_) | Value::Json(_) => {
                // Turso may parse JSON TEXT columns into Value::Object (or
                // Value::Json). Serialize back to string and compare canonically.
                fn canonical_json_from_value(v: &Value) -> String {
                    let json_val: serde_json::Value = value_to_json(v);
                    match json_val {
                        serde_json::Value::Object(map) => {
                            let sorted: std::collections::BTreeMap<_, _> =
                                map.into_iter().collect();
                            serde_json::to_string(&sorted)
                                .expect("BTreeMap serialization cannot fail")
                        }
                        other => serde_json::to_string(&other)
                            .expect("serde_json::Value serialization cannot fail"),
                    }
                }
                fn canonical_json_from_str(s: &str) -> Option<String> {
                    // Returns None when `s` is not valid JSON — treat as not-equal.
                    let parsed: serde_json::Value = serde_json::from_str(s)
                        .map_err(|e| {
                            tracing::warn!(
                                "sql_literal_equals_value: SQL literal is not valid JSON ({e}): {s:?}"
                            )
                        })
                        .ok()?; // ALLOW(ok): None means "not valid JSON → treat as changed"
                    Some(match parsed {
                        serde_json::Value::Object(map) => {
                            let sorted: std::collections::BTreeMap<_, _> =
                                map.into_iter().collect();
                            serde_json::to_string(&sorted)
                                .expect("BTreeMap serialization cannot fail")
                        }
                        other => serde_json::to_string(&other)
                            .expect("serde_json::Value serialization cannot fail"),
                    })
                }
                match canonical_json_from_str(&unescaped) {
                    Some(new_canonical) => new_canonical == canonical_json_from_value(db),
                    None => false,
                }
            }
            _ => false,
        };
    }
    // Numeric literal.
    if let Ok(n) = sql_literal.parse::<i64>() {
        return match db {
            Value::Integer(i) => *i == n,
            _ => false,
        };
    }
    if let Ok(f) = sql_literal.parse::<f64>() {
        return match db {
            Value::Float(g) => (*g - f).abs() < f64::EPSILON,
            _ => false,
        };
    }
    // Boolean literals.
    if sql_literal.eq_ignore_ascii_case("true") {
        return matches!(db, Value::Boolean(true));
    }
    if sql_literal.eq_ignore_ascii_case("false") {
        return matches!(db, Value::Boolean(false));
    }
    false
}

/// Known columns in the blocks table that can be used directly in SQL.
/// Any param key not in this set gets packed into the `properties` JSON column.
/// Known columns in the blocks table (must match schema in schema_modules.rs).
const BLOCKS_KNOWN_COLUMNS: &[&str] = &[
    "id",
    "parent_id",
    "depth",
    "sort_key",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "properties",
    "marks",
    "collapsed",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
    "_change_origin",
];

/// A prepared operation, split into two FK-ordered phases so a batch can apply
/// ALL block_raw rows before ANY edge junction (rows-then-edges). This makes the
/// op-vec order irrelevant for FK safety: a create batch containing a
/// `block_requires`/`advice_suppressed` pair can never insert the junction before
/// its referenced row exists — the root cause of the Face-A whole-batch rollback.
struct PreparedOp {
    /// `block_raw` row statements (INSERT/UPSERT/DELETE of the row itself).
    /// Order-independent within one transaction: `parent_id`'s self-FK is
    /// DEFERRABLE INITIALLY DEFERRED (checked at COMMIT), and the junction tables
    /// (`block_requires`/`block_tags`/`advice_suppressed`) are `ON DELETE
    /// CASCADE`, so deleting a row cleans up its junctions automatically.
    row_statements: Vec<String>,
    /// Junction/edge-table statements (`block_requires`/`block_tags`/
    /// `advice_suppressed`). Their FKs into `block_raw(id)` are IMMEDIATE, so they
    /// MUST run after every referenced `block_raw` row exists — i.e. after all
    /// `row_statements` of the whole batch.
    edge_statements: Vec<String>,
}

/// SQL-based operation provider that writes directly to a Turso table.
///
/// Uses the DbHandle actor to execute SQL, ensuring all writes go through
/// the connection that has CDC callbacks registered. This is critical for
/// materialized view change detection and real-time UI updates.
pub struct SqlOperationProvider {
    db_handle: DbHandle,
    table_name: String,
    entity_name: String,
    entity_short_name: String,
    known_columns: HashSet<String>,
    /// Edge-typed fields (multi-valued, projected to a junction table).
    /// Indexed by field name for O(1) partition-time lookup.
    edge_fields: HashMap<String, EdgeFieldDescriptor>,
    /// Wall-clock authority for write-time timestamps. Defaults to SystemClock.
    clock: std::sync::Arc<dyn holon_api::Clock>,
}

impl SqlOperationProvider {
    pub fn new(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
    ) -> Self {
        Self::with_edge_fields(
            db_handle,
            table_name,
            entity_name,
            entity_short_name,
            Vec::new(),
        )
    }

    /// Construct with an explicit edge-field registry (filtered to this entity).
    /// Descriptors whose `entity` doesn't match `entity_name` are dropped.
    pub fn with_edge_fields(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
        edge_fields: Vec<EdgeFieldDescriptor>,
    ) -> Self {
        let known_columns = BLOCKS_KNOWN_COLUMNS.iter().map(|s| s.to_string()).collect();
        let edge_fields = edge_fields
            .into_iter()
            .filter(|d| d.entity == entity_name)
            .map(|d| (d.field.clone(), d))
            .collect();
        Self {
            db_handle,
            table_name,
            entity_name,
            entity_short_name,
            known_columns,
            edge_fields,
            clock: std::sync::Arc::new(holon_api::SystemClock),
        }
    }

    /// Override the clock (tests). Defaults to SystemClock.
    pub fn with_clock(mut self, clock: std::sync::Arc<dyn holon_api::Clock>) -> Self {
        self.clock = clock;
        self
    }

    fn value_to_sql(value: &Value) -> String {
        value_to_sql_literal(value)
    }

    fn quote_identifier(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    /// Normalize a content value for org round-trip stability.
    ///
    /// For text blocks the first line becomes the org headline, which the
    /// parser `.trim()`s (both ends) on re-parse, so leading *and* trailing
    /// whitespace on the first line is stripped on re-ingest. Trailing
    /// whitespace on the whole string is also stripped. Source blocks
    /// preserve content verbatim (aside from overall trailing-whitespace
    /// trim) because their body is not remodeled as a headline.
    ///
    /// `is_source` selects between the two modes. Callers that don't know
    /// the type pass `false` — matches the common-case text path and
    /// mirrors `normalize_content_for_org_roundtrip` in `pbt/types.rs`.
    fn trimmed_content(value: &Value, is_source: bool) -> Value {
        match value {
            Value::String(s) => {
                let trimmed_end = s.trim_end();
                if is_source {
                    return Value::String(trimmed_end.to_string());
                }
                Value::String(match trimmed_end.split_once('\n') {
                    Some((first, rest)) => format!("{}\n{}", first.trim(), rest),
                    None => trimmed_end.trim_start().to_string(),
                })
            }
            other => other.clone(),
        }
    }

    /// Separate params into three buckets:
    /// 1. known SQL columns (folded directly into the row)
    /// 2. edge-typed fields (multi-valued, projected to a junction table —
    ///    their `Value::Array` payload is captured raw and routed through
    ///    DELETE+INSERT by the caller)
    /// 3. extra properties (merged into the `properties` JSON column)
    ///
    /// If params already contains a `properties` field, its JSON content is
    /// merged with the extra properties bucket.
    #[allow(clippy::type_complexity)]
    fn partition_params(
        &self,
        params: &StorageEntity,
    ) -> (
        Vec<(String, String)>,
        std::collections::HashMap<String, Value>,
        Vec<(EdgeFieldDescriptor, Vec<String>)>,
    ) {
        let mut sql_fields = Vec::new();
        let mut extra_props = std::collections::HashMap::new();
        let mut edge_field_params: Vec<(EdgeFieldDescriptor, Vec<String>)> = Vec::new();
        let mut existing_properties_json: Option<String> = None;

        // First-line headline trimming only applies to text blocks; source
        // blocks preserve content verbatim. Look up `content_type` from the
        // params so `trimmed_content` can branch correctly. Defaults to
        // text when absent — the common case.
        let is_source = params.get("content_type").and_then(|v| v.as_string()) == Some("source");

        for (key, value) in params.iter() {
            if &**key == "properties" {
                // Capture existing properties JSON to merge with extras later
                if let Some(s) = value.as_string() {
                    existing_properties_json = Some(s.to_string());
                }
            } else if &**key == POSITION_AFTER_BLOCK_ID_PARAM
                || key.starts_with("_routing_")
                || key.starts_with("_expected_")
            {
                // Operation-control metadata (positional intent, routing
                // hints, diff guards) — never persist. The positional
                // intent is lifted onto the typed `Event` field in
                // `prepare_create`. Other underscore-prefix keys (e.g.
                // `_source_header_args`, `_source_results`) are real block
                // properties and must flow through to `extra_props`.
            } else if let Some(descriptor) = self.edge_fields.get(key.as_ref()) {
                // Edge-typed field: must carry a Value::Array. Fail loud if
                // a caller mis-types this — silently flowing to JSON would
                // be the *exact* H5 bug we're closing.
                let arr = match value {
                    Value::Array(items) => items,
                    other => panic!(
                        "SqlOperationProvider: edge field '{}' on '{}' must be Value::Array, got {:?}",
                        key, self.entity_name, other
                    ),
                };
                let ids: Vec<String> = arr
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => panic!(
                            "SqlOperationProvider: edge field '{}' items must be Value::String, got {:?}",
                            key, other
                        ),
                    })
                    .collect();
                edge_field_params.push((descriptor.clone(), ids));
            } else if self.known_columns.contains(key.as_ref()) {
                // Trim trailing whitespace from content — org files don't
                // preserve it, so storing untrimmed content would cause
                // permanent divergence between DB and org round-trips.
                let value = if &**key == "content" {
                    &Self::trimmed_content(value, is_source)
                } else {
                    value
                };
                sql_fields.push((key.to_string(), Self::value_to_sql(value)));
            } else {
                extra_props.insert(key.to_string(), value.clone());
            }
        }

        // Merge existing properties JSON into extra_props
        if let Some(json_str) = existing_properties_json
            && let Ok(map) = serde_json::from_str::<
                std::collections::HashMap<String, serde_json::Value>,
            >(&json_str)
        {
            for (k, v) in map {
                extra_props.entry(k).or_insert_with(|| match v {
                    serde_json::Value::String(s) => Value::String(s),
                    serde_json::Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            Value::Integer(i)
                        } else {
                            Value::Float(n.as_f64().unwrap_or(0.0))
                        }
                    }
                    serde_json::Value::Bool(b) => Value::Boolean(b),
                    _ => Value::String(v.to_string()),
                });
            }
        }

        (sql_fields, extra_props, edge_field_params)
    }

    /// Build SQL statements that replace the edge-field rows for `id`.
    /// Always DELETE all current rows for the source then INSERT the new set
    /// — coarse but correct, and the H5 sizing showed this is acceptable for
    /// G1 (≤ ~10 blockers/tags per block).
    fn edge_field_replace_sql(
        id: &str,
        descriptor: &EdgeFieldDescriptor,
        targets: &[String],
    ) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!(
            "DELETE FROM {jt} WHERE {sc} = '{id}'",
            jt = descriptor.join_table,
            sc = Self::quote_identifier(&descriptor.source_col),
            id = id.replace('\'', "''"),
        ));
        for target in targets {
            out.push(format!(
                "INSERT INTO {jt} ({sc}, {tc}) VALUES ('{id}', '{tg}')",
                jt = descriptor.join_table,
                sc = Self::quote_identifier(&descriptor.source_col),
                tc = Self::quote_identifier(&descriptor.target_col),
                id = id.replace('\'', "''"),
                tg = target.replace('\'', "''"),
            ));
        }
        out
    }

    /// True when a Turso write error is a foreign-key-constraint failure
    /// (immediate or the deferred-at-commit variant). The fork's `LimboError`
    /// renders these as "... foreign key constraint failed ..." and names NO
    /// constraint, so this predicate cannot tell WHICH FK failed — callers that
    /// need to attribute a specific FK (e.g. the block parent) must confirm the
    /// referenced row's presence themselves (see `block_row_exists`).
    fn is_fk_violation(err_msg: &str) -> bool {
        err_msg.to_lowercase().contains("foreign key constraint")
    }

    /// Whether a `block_raw` row with `id` currently exists. Used to attribute a
    /// create-time FK failure accurately: the parent FK and the junction source
    /// FKs both surface the same opaque message, so "parent not found" must be
    /// proven, not assumed.
    async fn block_row_exists(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM {} WHERE id = '{}' LIMIT 1",
            self.table_name,
            id.replace('\'', "''"),
        );
        Ok(!self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("block_row_exists({id}): {e}"))?
            .is_empty())
    }

    /// The shared typed rejection for a block write whose parent FK failed.
    /// `child_id`/`parent_id` are the schemed strings from the operation params
    /// crossing back into typed form at this write-boundary error edge.
    fn parent_not_found(
        child_id: &str,
        parent_id: &str,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(ParentNotFound {
            // ALLOW(entity_uri_from_raw): SQL-op param string at the write boundary.
            parent_id: EntityUri::from_raw(parent_id),
            // ALLOW(entity_uri_from_raw): SQL-op param string at the write boundary.
            child_id: EntityUri::from_raw(child_id),
        })
    }

    /// Execute a prepared operation: run its SQL statements, rows before edges so
    /// a junction never precedes its referenced `block_raw` row.
    async fn execute_prepared(&self, prepared: PreparedOp) -> Result<()> {
        for sql in prepared
            .row_statements
            .iter()
            .chain(&prepared.edge_statements)
        {
            self.db_handle
                .execute(sql, vec![])
                .await
                .map_err(|e| format!("Failed to execute SQL: {}", e))?;
        }
        Ok(())
    }

    /// Build SQL for a create operation without executing.
    fn prepare_create(&self, params: &StorageEntity) -> PreparedOp {
        // Ensure timestamps are present so the event payload is a complete Block.
        // Without this, CacheEventSubscriber fails to deserialize: "missing field created_at".
        let mut params = params.clone();
        let now_ms = self.clock.now_millis();
        params
            .entry("created_at".into())
            .or_insert_with(|| Value::Integer(now_ms));
        params
            .entry("updated_at".into())
            .or_insert_with(|| Value::Integer(now_ms));

        let (mut sql_fields, extra_props, edge_field_params) = self.partition_params(&params);

        if !extra_props.is_empty() {
            // `Value::Null` props are removal sentinels (see `prepare_update`);
            // on create there is nothing to remove, so they are dropped instead
            // of serializing a literal JSON `null`.
            let props_json = properties_to_canonical_json(
                extra_props
                    .into_iter()
                    .filter(|(_, v)| !matches!(v, Value::Null))
                    .map(|(k, v)| (k, value_to_json(&v))),
            );
            sql_fields.push((
                "properties".to_string(),
                format!("'{}'", props_json.replace('\'', "''")),
            ));
        }

        let columns: Vec<_> = sql_fields
            .iter()
            .map(|(k, _)| Self::quote_identifier(k))
            .collect();
        let values: Vec<_> = sql_fields.iter().map(|(_, v)| v.clone()).collect();

        // UPSERT — a row with this id may already exist when the
        // `LoroSyncController::on_loro_changed` projector emits a Loro-origin
        // create after the org parser already inserted the SQL row.
        // INSERT OR IGNORE silently discards the projector's authoritative
        // Loro fi in that case, leaving SQL `sort_key` stuck on the parser's
        // value and breaking sibling ordering after `mov_after`. UPSERT lets
        // the projector be the single authoritative writer for `sort_key`
        // and other Loro-derived columns. The `id` column is the conflict
        // target so it's excluded from the SET clause.
        let upsert_set: Vec<String> = sql_fields
            .iter()
            .filter(|(k, _)| k != "id")
            .map(|(k, _)| {
                let q = Self::quote_identifier(k);
                format!("{q} = excluded.{q}")
            })
            .collect();
        let row_statements = vec![format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET {}",
            self.table_name,
            columns.join(", "),
            values.join(", "),
            upsert_set.join(", "),
        )];

        let aggregate_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or_default();

        // Edge-field rows: clear and reinsert per descriptor (no-op when no
        // edge fields are declared on this entity).
        let mut edge_statements = Vec::new();
        for (descriptor, targets) in &edge_field_params {
            edge_statements.extend(Self::edge_field_replace_sql(
                aggregate_id,
                descriptor,
                targets,
            ));
        }

        PreparedOp {
            row_statements,
            edge_statements,
        }
    }

    /// Build SQL for an update operation without executing.
    /// Returns None if there are no fields to update.
    ///
    /// Async because it reads the existing row to merge `properties` JSON and to
    /// run the per-column diff guard that suppresses no-op UPDATEs.
    async fn prepare_update(&self, params: &StorageEntity) -> Result<Option<PreparedOp>> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("SqlOperationProvider::prepare_update: missing 'id' parameter");

        // Phase 2 authority flip: ALL `_expected_*` watermark guards are
        // gone. `SqlBlockOperations::set_field` routes block field writes
        // through `BlockCellRegistry::write_field` (Loro), and
        // `LoroSyncController::on_loro_changed` is the only path that
        // writes block columns to SQL. With a single writer per field
        // there's no concurrent direct SQL dispatch to regress against,
        // so the compare-and-set is dead weight. The diff guard at the
        // end of this function still keeps no-op UPDATEs from firing
        // spurious CDC.

        let (sql_fields, extra_props, edge_field_params) = self.partition_params(params);

        // TRACE: any non-standard custom property being written via update path
        const STANDARD_PROP_KEYS: &[&str] = &[
            "task_state",
            "task_state_category",
            "priority",
            "tags",
            "scheduled",
            "deadline",
            "sequence",
            "level",
            "ID",
            "org_properties",
        ];
        let custom_keys: Vec<&String> = extra_props
            .keys()
            .filter(|k| !STANDARD_PROP_KEYS.contains(&k.as_str()) && !k.starts_with('_'))
            .collect();
        if !custom_keys.is_empty() {
            tracing::trace!(
                "[CUSTOMPROP-TRACE prepare_update] id={id} custom_keys={:?} extra_props={:?} sql_fields_keys={:?}",
                custom_keys,
                extra_props,
                sql_fields.iter().map(|(k, _)| k).collect::<Vec<_>>()
            );
        }

        // Collect (column, sql_value) pairs for all modified columns.
        // Used to build both SET clauses and the diff guard WHERE condition.
        let mut update_pairs: Vec<(String, String)> = sql_fields
            .iter()
            .filter(|(k, _)| k != "id")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if !extra_props.is_empty() {
            // Read existing properties from DB and merge in Rust, then write the
            // full JSON string back. Turso IVM can't track json_set() changes —
            // it fires CDC with the OLD column value. Full replacement ensures
            // IVM sees the actual new value.
            let select_props_sql = format!(
                "SELECT properties FROM {} WHERE id = '{}'",
                self.table_name,
                id.replace('\'', "''")
            );
            let rows = self
                .db_handle
                .query(&select_props_sql, HashMap::new())
                .await
                .map_err(|e| {
                    format!("prepare_update: read existing properties for {}: {}", id, e)
                })?;
            let mut existing: serde_json::Map<String, serde_json::Value> = match rows
                .into_iter()
                .next()
            {
                None => serde_json::Map::new(),
                Some(row) => match row.get("properties").cloned() {
                    None | Some(Value::Null) => serde_json::Map::new(),
                    Some(Value::String(s)) if s.is_empty() => serde_json::Map::new(),
                    Some(Value::String(s)) => serde_json::from_str(&s).map_err(|e| {
                        format!(
                            "prepare_update: properties column for {} is not valid JSON ({}): {:?}",
                            id, e, s
                        )
                    })?,
                    Some(Value::Object(m)) => {
                        m.into_iter().map(|(k, v)| (k, value_to_json(&v))).collect()
                    }
                    Some(other) => {
                        return Err(format!(
                            "prepare_update: properties column for {} has unexpected type: {:?}",
                            id, other
                        )
                        .into());
                    }
                },
            };

            for (k, v) in &extra_props {
                // `Value::Null` is the property-REMOVAL sentinel: a merge that
                // only ever inserts can never clear a stale key (e.g. a
                // `#+TODO:` keyword set deleted from the org file header).
                if matches!(v, Value::Null) {
                    existing.remove(k);
                } else {
                    existing.insert(k.clone(), value_to_json(v));
                }
            }

            // Canonicalize key order so the diff guard's string comparison
            // matches regardless of insertion order across code paths.
            let merged_json = properties_to_canonical_json(existing);
            let props_sql = format!("'{}'", merged_json.replace('\'', "''"));
            update_pairs.push(("properties".to_string(), props_sql));
        }

        if update_pairs.is_empty() && edge_field_params.is_empty() {
            return Ok(None);
        }

        // Rust per-column diff: read the OLD row once and drop pairs whose
        // value hasn't changed. This replaces the SQL `IS NOT` chain which
        // still fired an application-level Event even when 0 rows were touched.
        //
        // DIFF_GUARD_SKIP columns (`updated_at`, `created_at`) are always
        // regenerated to `now` — we keep them in the SET clause when real
        // content changed, but if they're the ONLY remaining pairs after
        // equality-dropping the others, we return None (timestamp-only update
        // is not a meaningful change and must not publish an Event).
        const DIFF_GUARD_SKIP: &[&str] = &["updated_at", "created_at"];
        if !update_pairs.is_empty() {
            let col_names: Vec<String> = update_pairs
                .iter()
                .filter(|(k, _)| !DIFF_GUARD_SKIP.contains(&k.as_str()))
                .map(|(k, _)| Self::quote_identifier(k))
                .collect();
            if !col_names.is_empty() {
                let select_sql = format!(
                    "SELECT {} FROM {} WHERE id = '{}'",
                    col_names.join(", "),
                    self.table_name,
                    id.replace('\'', "''")
                );
                let rows = self
                    .db_handle
                    .query(&select_sql, HashMap::new())
                    .await
                    .map_err(|e| format!("prepare_update: diff read for {}: {}", id, e))?;
                if let Some(old_row) = rows.into_iter().next() {
                    update_pairs.retain(|(k, new_sql_literal)| {
                        if DIFF_GUARD_SKIP.contains(&k.as_str()) {
                            return true; // keep timestamps — pruned later if nothing else changed
                        }
                        let old_val = old_row.get(k.as_str());
                        !sql_literal_equals_value(new_sql_literal, old_val)
                    });
                }
            }
            // After equality-dropping: if only skip-list columns remain AND
            // there are no edge-field changes, the operation is a no-op.
            let has_meaningful = update_pairs
                .iter()
                .any(|(k, _)| !DIFF_GUARD_SKIP.contains(&k.as_str()));
            if !has_meaningful && edge_field_params.is_empty() {
                return Ok(None);
            }
        }

        let set_clauses: Vec<String> = update_pairs
            .iter()
            .map(|(k, v)| format!("{} = {}", Self::quote_identifier(k), v))
            .collect();

        let where_clause = format!("id = '{}'", id.replace('\'', "''"));

        let mut row_statements = Vec::new();
        if !update_pairs.is_empty() {
            row_statements.push(format!(
                "UPDATE {} SET {} WHERE {}",
                self.table_name,
                set_clauses.join(", "),
                where_clause,
            ));
        }
        let mut edge_statements = Vec::new();
        for (descriptor, targets) in &edge_field_params {
            edge_statements.extend(Self::edge_field_replace_sql(id, descriptor, targets));
        }
        Ok(Some(PreparedOp {
            row_statements,
            edge_statements,
        }))
    }

    /// Build SQL for a delete operation (with cascade) without executing.
    /// Requires async because cascade discovery queries the DB.
    async fn prepare_delete(&self, params: &StorageEntity) -> Result<PreparedOp> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        let mut queue = vec![id.to_string()];
        let mut all_ids = Vec::new();
        // Cycle guard: a block tree has exactly one parent per block, so a
        // block appearing twice in a descendant walk means a parent cycle
        // (e.g. a self-referential `parent_id == id` row). Without this the
        // cascade `queue.extend(children)` loops forever. Fail loud — a cycle
        // is corrupt state, not a recoverable condition.
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(id.to_string());
        while let Some(parent) = queue.pop() {
            let children_sql = format!(
                "SELECT id FROM {} WHERE parent_id = '{}'",
                self.table_name,
                parent.replace('\'', "''")
            );
            let children: Vec<String> = self
                .db_handle
                .query(&children_sql, HashMap::new())
                .await
                .map_err(|e| format!("Failed to query children: {}", e))?
                .into_iter()
                .filter_map(|row| {
                    row.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                })
                .collect();
            for child in children {
                if !visited.insert(child.clone()) {
                    return Err(format!(
                        "prepare_delete: parent cycle detected while cascading delete of \
                         '{id}' — block '{child}' is its own ancestor (corrupt block tree)"
                    )
                    .into());
                }
                queue.push(child.clone());
                all_ids.push(child);
            }
        }

        // All statements are `block_raw` row DELETEs; the junction tables are
        // `ON DELETE CASCADE`, so removing the row cleans up its edges. No
        // `edge_statements` needed.
        let mut row_statements = Vec::new();

        // Delete descendants bottom-up
        for desc_id in all_ids.iter().rev() {
            row_statements.push(format!(
                "DELETE FROM {} WHERE id = '{}'",
                self.table_name,
                desc_id.replace('\'', "''")
            ));
        }

        // Delete the target block itself
        row_statements.push(format!(
            "DELETE FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''")
        ));

        Ok(PreparedOp {
            row_statements,
            edge_statements: Vec::new(),
        })
    }
}

#[async_trait]
impl OperationProvider for SqlOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "set_field".to_string(),
                display_name: "Set Field".to_string(),
                description: format!("Set a field on {}", self.entity_short_name),
                required_params: vec![
                    OperationParam {
                        name: "id".to_string(),
                        type_hint: TypeHint::String,
                        description: "Entity ID".to_string(),
                    },
                    OperationParam {
                        name: "field".to_string(),
                        type_hint: TypeHint::String,
                        description: "Field name".to_string(),
                    },
                    OperationParam {
                        name: "value".to_string(),
                        type_hint: TypeHint::String,
                        description: "Field value".to_string(),
                    },
                ],
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "create".to_string(),
                display_name: "Create".to_string(),
                description: format!("Create a new {}", self.entity_short_name),
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "update".to_string(),
                display_name: "Update".to_string(),
                description: format!("Update {}", self.entity_short_name),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Entity ID".to_string(),
                }],
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "delete".to_string(),
                display_name: "Delete".to_string(),
                description: format!("Delete {}", self.entity_short_name),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Entity ID".to_string(),
                }],
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "cycle_task_state".to_string(),
                display_name: "Cycle Task State".to_string(),
                description: "Cycle to the next task state".to_string(),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Entity ID".to_string(),
                }],
                affected_fields: vec!["task_state".to_string()],
                ..Default::default()
            },
        ]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        // Single-op writes that go through the trait's `execute_operation`
        // (rather than `execute_operation_with_origin`) inherit the legacy
        // `Other("sql")` origin. The inbound gate drops these by default
        // (Phase 3.3 step 2); migrate callers to
        // `execute_operation_with_origin(.., EventOrigin::Org)` (or another
        // whitelisted origin) to make their events reach Loro.
        self.execute_operation_with_origin(
            entity_name,
            op_name,
            params,
            EventOrigin::Other("sql".to_string()),
        )
        .await
    }
}

#[async_trait]
impl OriginTaggedWrites for SqlOperationProvider {
    async fn execute_operation_with_origin(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: EventOrigin,
    ) -> Result<OperationResult> {
        assert_eq!(
            entity_name.as_str(),
            self.entity_name.as_str(),
            "SqlOperationProvider: expected entity '{}', got '{}'",
            self.entity_name,
            entity_name
        );

        match op_name {
            "set_field" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'id' parameter".to_string())?;
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'field' parameter".to_string())?;
                let raw_value = params
                    .get("value")
                    .ok_or_else(|| "Missing 'value' parameter".to_string())?;
                let value = if field == "content" {
                    // For set_field, params only carries {id, field, value} — no
                    // content_type. Look up the existing block's content_type so
                    // source blocks preserve first-line whitespace verbatim.
                    let ct_sql = format!(
                        "SELECT content_type FROM {} WHERE id = '{}'",
                        self.table_name,
                        id.replace('\'', "''")
                    );
                    let rows = self
                        .db_handle
                        .query(&ct_sql, HashMap::new())
                        .await
                        .map_err(|e| format!("set_field content_type lookup failed: {}", e))?;
                    let is_source = rows
                        .into_iter()
                        .next()
                        .and_then(|row| {
                            row.get("content_type")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .is_some_and(|s| s == "source");
                    Self::trimmed_content(raw_value, is_source)
                } else {
                    raw_value.clone()
                };

                let sql_value = Self::value_to_sql(&value);

                // Edge-typed field: DELETE all current rows then INSERT new
                // ones (route through prepare-style helper so set_field
                // honours the same junction-table contract as create/update).
                if let Some(descriptor) = self.edge_fields.get(field) {
                    let empty: Vec<Value> = Vec::new();
                    let arr: &Vec<Value> = match &value {
                        Value::Array(items) => items,
                        Value::Null => &empty,
                        other => {
                            return Err(format!(
                                "set_field for edge '{}' must be Value::Array, got {:?}",
                                field, other
                            )
                            .into());
                        }
                    };
                    let targets: Vec<String> = arr
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => Ok(s.clone()),
                            other => Err(format!(
                                "set_field for edge '{}' items must be Value::String, got {:?}",
                                field, other
                            )),
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    for stmt in Self::edge_field_replace_sql(id, descriptor, &targets) {
                        self.db_handle
                            .execute(&stmt, vec![])
                            .await
                            .map_err(|e| format!("Failed to execute edge-field SQL: {}", e))?;
                    }
                    return Ok(OperationResult::irreversible(Vec::new()));
                }

                let sql = if self.known_columns.contains(field) {
                    format!(
                        "UPDATE {} SET {} = {} WHERE id = '{}'",
                        self.table_name,
                        Self::quote_identifier(field),
                        sql_value,
                        id.replace('\'', "''")
                    )
                } else if matches!(value, Value::Null) {
                    // Null means "remove this property" — use json_remove so we don't
                    // leave a {"key": null} entry in the JSON column. `task_state`
                    // removal also removes its `task_state_category` sidecar (the
                    // pair invariant `Block::set_task_state` establishes).
                    if field == "task_state" {
                        format!(
                            "UPDATE {} SET properties = json_remove(COALESCE(properties, '{{}}'), '$.task_state', '$.task_state_category') WHERE id = '{}'",
                            self.table_name,
                            id.replace('\'', "''")
                        )
                    } else {
                        format!(
                            "UPDATE {} SET properties = json_remove(COALESCE(properties, '{{}}'), '$.{}') WHERE id = '{}'",
                            self.table_name,
                            field.replace('\'', "''"),
                            id.replace('\'', "''")
                        )
                    }
                } else if field == "task_state" {
                    // A bare keyword write gets its `task_state_category` sidecar
                    // derived and written in the SAME statement — otherwise every
                    // UI cycle dropped/staled the category and a DONE keyword could
                    // read back as Active (see `TaskState::category_str_for_keyword`).
                    let keyword = value.as_string().ok_or_else(|| {
                        format!("set_field('task_state'): expected String or Null, got {value:?}")
                    })?;
                    let category = holon_api::TaskState::category_str_for_keyword(keyword);
                    format!(
                        "UPDATE {} SET properties = json_set(COALESCE(properties, '{{}}'), '$.task_state', {}, '$.task_state_category', '{}') WHERE id = '{}'",
                        self.table_name,
                        sql_value,
                        category,
                        id.replace('\'', "''")
                    )
                } else {
                    format!(
                        "UPDATE {} SET properties = json_set(COALESCE(properties, '{{}}'), '$.{}', {}) WHERE id = '{}'",
                        self.table_name,
                        field.replace('\'', "''"),
                        sql_value,
                        id.replace('\'', "''")
                    )
                };
                // Reparenting writes `parent_id`, which the deferred block FK
                // checks at COMMIT. Run it in a transaction so a rejected
                // reparent ROLLS BACK (autocommit would leave the bad parent_id
                // written despite the raised error). Other columns keep the
                // cheaper autocommit path — none carry an FK.
                let exec_res = if field == "parent_id" {
                    self.db_handle
                        .transaction(vec![(sql.clone(), vec![])])
                        .await
                } else {
                    self.db_handle.execute(&sql, vec![]).await.map(|_| ())
                };
                if let Err(e) = exec_res {
                    let msg = e.to_string();
                    // This UPDATE writes ONLY the `parent_id` column, whose sole
                    // FK is the block parent — so a FK failure here is
                    // unambiguously the parent (unlike the multi-FK create path).
                    if field == "parent_id" && Self::is_fk_violation(&msg) {
                        let parent = value.as_string().unwrap_or_default();
                        return Err(Self::parent_not_found(id, parent));
                    }
                    return Err(format!("Failed to execute SQL: {}", msg).into());
                }

                if field == "content" {
                    let verify_sql = format!(
                        "SELECT content FROM {} WHERE id = '{}'",
                        self.table_name,
                        id.replace('\'', "''")
                    );
                    let rows = self
                        .db_handle
                        .query(&verify_sql, HashMap::new())
                        .await
                        .unwrap_or_default();
                    let after_content = rows
                        .first()
                        .and_then(|r| r.get("content"))
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string();
                    tracing::trace!(
                        "[SET_FIELD_TRACE] id={} post-UPDATE content={:?} (wrote={:?})",
                        id,
                        after_content,
                        value.as_string().unwrap_or("")
                    );
                }

                Ok(OperationResult::irreversible(Vec::new()))
            }
            "create" => {
                // Entity ids are `{entity}:{uuid}`. A create without an explicit
                // id means "mint a fresh one" — the normal case for interactive
                // block creation and Rhai `block.create(..)` actions. Only split
                // and seeds supply an id. Mint here (mirroring `split_block`) so
                // the op's postcondition — a row with a valid id — holds for every
                // caller, instead of panicking a background worker whose crash the
                // UI silently swallows.
                let mut params = params;
                let id = match params.get("id").and_then(|v| v.as_string()) {
                    Some(existing) => existing.to_string(),
                    None => {
                        let minted = format!("{}:{}", self.entity_name, uuid::Uuid::new_v4());
                        params.insert("id".into(), Value::String(minted.clone()));
                        minted
                    }
                };
                let prepared = self.prepare_create(&params);
                // Run the create atomically in one transaction. The block parent
                // FK is DEFERRABLE INITIALLY DEFERRED, so it is checked at COMMIT.
                // A transaction (unlike an autocommit statement) ROLLS BACK the
                // offending row on that commit-time failure, so a rejected create
                // leaves no partial row — integrity, not just a loud error.
                let mut stmts = Vec::new();
                stmts.extend(
                    prepared
                        .row_statements
                        .iter()
                        .chain(&prepared.edge_statements)
                        .map(|s| (s.clone(), vec![])),
                );
                if let Err(e) = self.db_handle.transaction(stmts).await {
                    let msg = e.to_string();
                    if Self::is_fk_violation(&msg) {
                        let parent = params
                            .get("parent_id")
                            .and_then(|v| v.as_string())
                            .unwrap_or_default();
                        // A create transaction enforces TWO kinds of FK: the
                        // block's own parent FK (deferred, checked at COMMIT) and
                        // the junction/edge SOURCE FKs. The fork's error text is
                        // only "FOREIGN KEY constraint failed" — it names no
                        // constraint — so we must not assume it was the parent.
                        // Blindly mapping every create-time FK failure to
                        // `ParentNotFound` sent TWO debugging rounds down the
                        // wrong path (dogfood 2026-07-10: the real cause was a
                        // dangling `block_requires.required_id` target FK, yet the
                        // error claimed the parent — which existed — was missing).
                        // Attribute accurately: only claim `ParentNotFound` when
                        // the parent row is genuinely absent; otherwise surface a
                        // precise error naming the SQL that actually failed.
                        let parent_present =
                            !parent.is_empty() && self.block_row_exists(parent).await?;
                        if !parent_present {
                            return Err(Self::parent_not_found(&id, parent));
                        }
                        return Err(format!(
                            "create for {id}: a foreign-key constraint failed but the parent \
                             {parent} EXISTS — a junction/edge source FK or another constraint \
                             rejected the write, NOT the parent. Failing statements: {:#?}. \
                             Underlying: {msg}",
                            prepared
                                .row_statements
                                .iter()
                                .chain(&prepared.edge_statements)
                                .collect::<Vec<_>>()
                        )
                        .into());
                    }
                    return Err(format!("Failed to execute SQL: {}", msg).into());
                }
                // After INSERT OR IGNORE, read back the actual row to detect
                // whether the insert was ignored (duplicate name+parent_id).
                // Return the actual DB id so the caller knows which UUID won.
                let select_sql = format!(
                    "SELECT id FROM {} WHERE id = '{}'",
                    self.table_name,
                    id.replace('\'', "''")
                );
                let inserted = match self.db_handle.query(&select_sql, HashMap::new()).await {
                    Ok(rows) => rows.into_iter().next().is_some(),
                    Err(e) => {
                        tracing::error!(
                            "[SqlOp] SELECT after INSERT failed for '{}': {} — treating as not inserted",
                            id,
                            e,
                        );
                        false
                    }
                };

                let response = if !inserted {
                    // Our id doesn't exist → INSERT was ignored. With the unique
                    // (parent_id, name) index gone, this branch only triggers on
                    // primary-key collision; resolve by id alone.
                    let block_id = params.get("id").and_then(|v| v.as_string());
                    match block_id {
                        Some(bid) => {
                            let find_sql = format!(
                                "SELECT id FROM {} WHERE id = '{}'",
                                self.table_name,
                                bid.replace('\'', "''"),
                            );
                            let existing_id = self
                                .db_handle
                                .query(&find_sql, HashMap::new())
                                .await
                                .ok() // ALLOW(ok): id-collision lookup tolerance // ALLOW(fallback): pre-existing comment-only mention; not a real fallback.
                                .and_then(|rows| rows.into_iter().next())
                                .and_then(|row| row.get("id").cloned());
                            existing_id.map(|v| match v {
                                Value::String(s) => Value::String(s),
                                other => Value::String(format!("{:?}", other)),
                            })
                        }
                        _ => None,
                    }
                } else {
                    None
                };

                let mut result = OperationResult::irreversible(Vec::new());
                result.response = response;
                Ok(result)
            }
            "update" => {
                if let Some(prepared) = self.prepare_update(&params).await? {
                    self.execute_prepared(prepared).await?;
                }
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "delete" => {
                let prepared = self.prepare_delete(&params).await?;
                self.execute_prepared(prepared).await?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "cycle_task_state" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'id' parameter".to_string())?
                    .to_string();

                let sql = format!(
                    "SELECT json_extract(properties, '$.task_state') as task_state FROM {} WHERE id = '{}'",
                    self.table_name,
                    id.replace('\'', "''")
                );
                let rows = self
                    .db_handle
                    .query(&sql, HashMap::new())
                    .await
                    .map_err(|e| format!("Failed to read task_state: {e}"))?;
                let current = rows
                    .first()
                    .and_then(|r| r.get("task_state"))
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();

                let states: Vec<String> =
                    vec!["".into(), "TODO".into(), "DOING".into(), "DONE".into()];
                let next = holon_api::render_eval::cycle_state(current, &states);

                // `set_field("task_state")` pairs the `task_state_category`
                // sidecar in the same UPDATE (see the set_field arm), keeping
                // the pair invariant `Block::set_task_state` establishes.
                let mut set_params = StorageEntity::new();
                set_params.insert("id".into(), Value::String(id));
                set_params.insert("field".into(), Value::String("task_state".into()));
                set_params.insert("value".into(), Value::String(next));
                self.execute_operation_with_origin(entity_name, "set_field", set_params, origin)
                    .await
            }
            _ => Err(format!("Unknown operation: {}", op_name).into()),
        }
    }

    /// Execute a batch in a single transaction.
    ///
    /// The `origin` argument is part of the `OriginTaggedWrites` write API
    /// (callers such as `LoroSyncController` tag their outbound batches
    /// `EventOrigin::Loro`), but the SQL writer no longer consumes it:
    /// provenance for echo-suppression rides the `_change_origin` CDC column via
    /// the trace context, not the (now-removed) EventBus.
    async fn execute_batch_with_origin(
        &self,
        entity_name: &EntityName,
        operations: Vec<(String, StorageEntity)>,
        _: EventOrigin,
    ) -> Result<Vec<OperationResult>> {
        assert_eq!(
            entity_name.as_str(),
            self.entity_name.as_str(),
            "SqlOperationProvider: expected entity '{}', got '{}'",
            self.entity_name,
            entity_name
        );

        if operations.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: Prepare all operations (may involve async DB reads for delete
        // cascade), collecting block_raw ROW statements separately from EDGE
        // (junction) statements.
        let mut row_sql: Vec<String> = Vec::new();
        let mut edge_sql: Vec<String> = Vec::new();

        for (op_name, params) in &operations {
            let prepared = match op_name.as_str() {
                "create" => self.prepare_create(params),
                "update" => match self.prepare_update(params).await? {
                    Some(p) => p,
                    None => continue,
                },
                "delete" => self.prepare_delete(params).await?,
                other => return Err(format!("Unknown batch operation: {}", other).into()),
            };
            row_sql.extend(prepared.row_statements);
            edge_sql.extend(prepared.edge_statements);
        }

        // Rows-then-edges: every `block_raw` row of the WHOLE batch is written
        // before any junction row, so a junction's IMMEDIATE FK into `block_raw`
        // always finds its target regardless of op-vec order. Row ordering among
        // themselves is safe (deferred `parent_id` self-FK settles at COMMIT;
        // junction cleanup on delete is `ON DELETE CASCADE`). This is the
        // structural fix for the Face-A whole-batch rollback.
        let all_sql: Vec<_> = row_sql
            .into_iter()
            .chain(edge_sql)
            .map(|s| (s, vec![]))
            .collect();

        let count = operations.len();

        // Phase 2: Execute all SQL in a single transaction
        tracing::info!(
            "[SqlOperationProvider] Executing batch: {} operations, {} SQL statements",
            count,
            all_sql.len()
        );
        let _tx_t0 = std::time::Instant::now();
        let _sql_count = all_sql.len();
        self.db_handle
            .transaction(all_sql)
            .await
            .map_err(|e| format!("Batch transaction failed: {}", e))?;
        tracing::info!(
            "[SqlOperationProvider] batch timing: {} ops, {} sql stmts → tx {}ms",
            count,
            _sql_count,
            _tx_t0.elapsed().as_millis(),
        );

        Ok(vec![OperationResult::irreversible(Vec::new()); count])
    }
}

#[cfg(test)]
#[path = "sql_operation_provider_diff_test.rs"]
mod sql_operation_provider_diff_test;

#[cfg(test)]
mod clock_tests {
    use super::*;
    use holon_api::TestClock;
    use std::sync::Arc;

    /// An injected clock drives the write-time `created_at`/`updated_at`
    /// timestamps instead of the ambient system clock.
    #[tokio::test]
    async fn with_clock_stamps_injected_timestamp() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        db_handle
            .execute(
                "CREATE TABLE block_raw (id TEXT PRIMARY KEY, created_at INTEGER, updated_at INTEGER)",
                vec![],
            )
            .await
            .expect("create table");

        let provider = SqlOperationProvider::new(
            db_handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
        )
        .with_clock(Arc::new(TestClock::new(123456)));

        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String("b1".to_string()));
        let prepared = provider.prepare_create(&params);

        db_handle
            .execute(&prepared.row_statements[0], vec![])
            .await
            .expect("insert");

        let rows = db_handle
            .query(
                "SELECT created_at, updated_at FROM block_raw WHERE id = 'b1'",
                HashMap::new(),
            )
            .await
            .expect("query");
        let row = &rows[0];
        assert_eq!(row.get("created_at"), Some(&Value::Integer(123456)));
        assert_eq!(row.get("updated_at"), Some(&Value::Integer(123456)));
    }
}

#[cfg(test)]
mod create_id_tests {
    use super::*;

    /// A `create` op with no `id` must MINT a fresh `{entity}:{uuid}` id and
    /// insert the row — never panic. Regression for the dogfood P1 where the
    /// GPUI creation-slot commit and the journal auto-create Rhai action both
    /// build `block.create` without an id and the provider `.expect`-panicked
    /// on a background worker the UI silently swallowed.
    #[tokio::test]
    async fn create_without_id_mints_a_block_scoped_id() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        db_handle
            .execute(
                "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
                 created_at INTEGER, updated_at INTEGER)",
                vec![],
            )
            .await
            .expect("create table");

        let provider = SqlOperationProvider::new(
            db_handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
        );

        let mut params = StorageEntity::new();
        params.insert("content".into(), Value::String("hello world".to_string()));
        // Deliberately no "id" — mirrors the creation-slot / action-watcher paths.
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "create",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect("create without an explicit id should mint one, not panic");

        let rows = db_handle
            .query("SELECT id, content FROM block_raw", HashMap::new())
            .await
            .expect("query");
        assert_eq!(rows.len(), 1, "exactly one row should have been inserted");
        let id = rows[0]
            .get("id")
            .and_then(|v| v.as_string())
            .expect("minted id present");
        let uuid_part = id
            .strip_prefix("block:")
            .unwrap_or_else(|| panic!("minted id should be block-scoped, got {id:?}"));
        assert!(
            uuid::Uuid::parse_str(uuid_part).is_ok(),
            "minted id suffix should be a uuid, got {uuid_part:?}"
        );
    }
}
#[cfg(test)]
mod two_phase_fk_tests {
    use super::*;
    use std::collections::HashMap;

    /// Face-A regression: a create batch containing a `requires` pair — the
    /// DEPENDENT block ordered BEFORE its required target — must apply FK-clean.
    /// Before the rows-then-edges two-phase split, the dependent's junction INSERT
    /// ran before the target's `block_raw` row existed, FK-rejecting and rolling
    /// back the WHOLE transaction (losing BOTH blocks). The two-phase apply writes
    /// every `block_raw` row before any junction, so op-vec order is irrelevant.
    #[tokio::test]
    async fn requires_pair_batch_applies_fk_clean_regardless_of_op_order() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        for ddl in [
            "PRAGMA foreign_keys = ON",
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, content TEXT, \
             created_at INTEGER, updated_at INTEGER)",
            "CREATE TABLE block_requires (block_id TEXT NOT NULL, required_id TEXT NOT NULL, \
             PRIMARY KEY (block_id, required_id), \
             FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE, \
             FOREIGN KEY (required_id) REFERENCES block_raw(id) ON DELETE CASCADE)",
        ] {
            db_handle.execute(ddl, vec![]).await.expect("ddl");
        }

        // Precondition: FK enforcement is actually live in this environment —
        // a junction row referencing absent blocks must be REJECTED. Without
        // this, the test below would pass vacuously.
        db_handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) \
                 VALUES ('block:ghostA', 'block:ghostB')",
                vec![],
            )
            .await
            .expect_err("FK enforcement must be ON for this test to be meaningful");

        let requires_descriptor = EdgeFieldDescriptor {
            entity: "block".to_string(),
            field: "requires".to_string(),
            join_table: "block_requires".to_string(),
            source_col: "block_id".to_string(),
            target_col: "required_id".to_string(),
        };
        let provider = SqlOperationProvider::with_edge_fields(
            db_handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
            vec![requires_descriptor],
        );

        // Adversarial op order: dependent A (requires B) emitted BEFORE B.
        let mut a = StorageEntity::new();
        a.insert("id".into(), Value::String("block:A".to_string()));
        a.insert("content".into(), Value::String("A".to_string()));
        a.insert(
            "requires".into(),
            Value::Array(vec![Value::String("block:B".to_string())]),
        );
        let mut b = StorageEntity::new();
        b.insert("id".into(), Value::String("block:B".to_string()));
        b.insert("content".into(), Value::String("B".to_string()));

        let ops = vec![("create".to_string(), a), ("create".to_string(), b)];
        provider
            .execute_batch_with_origin(&EntityName::new("block"), ops, EventOrigin::Loro)
            .await
            .expect("two-phase batch must apply FK-clean despite dependent-first order");

        let blocks = db_handle
            .query("SELECT id FROM block_raw ORDER BY id", HashMap::new())
            .await
            .expect("query blocks");
        assert_eq!(blocks.len(), 2, "both blocks must survive the batch");
        let junction = db_handle
            .query(
                "SELECT block_id, required_id FROM block_requires",
                HashMap::new(),
            )
            .await
            .expect("query junction");
        assert_eq!(junction.len(), 1, "the requires edge must be persisted");
    }
}
