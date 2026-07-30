//! SQL-based operation provider for blocks
//!
//! Provides direct SQL access to block operations, bypassing the Loro CRDT
//! layer. Used when OrgMode is enabled but Loro is disabled, or by any
//! component that needs to write blocks directly to the database.

use std::collections::HashMap;
use std::collections::HashSet;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::PAGE_TAG;
use holon_api::ParentNotFound;
use holon_api::TypeHint;
use holon_api::Value;
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

use crate::core::merge_blocks_plan;
use crate::storage::schema_module::EdgeFieldDescriptor;
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::DbHandle;
use crate::sync::event_bus::EventOrigin;
use crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM;

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
/// Used by `prepare_update`'s Rust diff guard to drop pairs that haven't
/// changed without relying on Turso's `IS NOT` string-comparison semantics.
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
                                "sql_literal_equals_value: SQL literal is not valid JSON ({e}): \
                                 {s:?}"
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
    "write_seq",
];

/// A prepared operation, split into two FK-ordered phases so a batch can apply
/// ALL block_raw rows before ANY edge junction (rows-then-edges). This makes
/// the op-vec order irrelevant for FK safety: a create batch containing a
/// `block_requires`/`advice_suppressed` pair can never insert the junction
/// before its referenced row exists — the root cause of the Face-A whole-batch
/// rollback.
struct PreparedOp {
    /// `block_raw` row statements (INSERT/UPSERT/DELETE of the row itself).
    /// Order-independent within one transaction: `parent_id`'s self-FK is
    /// DEFERRABLE INITIALLY DEFERRED (checked at COMMIT), and the junction
    /// tables (`block_requires`/`block_tags`/`advice_suppressed`) are `ON
    /// DELETE CASCADE`, so deleting a row cleans up its junctions
    /// automatically.
    row_statements: Vec<String>,
    /// Junction/edge-table statements (`block_requires`/`block_tags`/
    /// `advice_suppressed`). Their FKs into `block_raw(id)` are IMMEDIATE, so
    /// they MUST run after every referenced `block_raw` row exists — i.e.
    /// after all `row_statements` of the whole batch.
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

    /// Construct with an explicit edge-field registry (filtered to this
    /// entity). Descriptors whose `entity` doesn't match `entity_name` are
    /// dropped.
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
            // SINGLE SOURCE OF TRUTH: the exact transform lives in
            // `holon_api::content_canonical` so the GPUI editor's echo-suppression
            // discriminator (`evaluate_data_sync_echo`) can recognize the
            // canonicalized echo of its own write without the two definitions
            // drifting (a drift would let the store canonicalize typed whitespace
            // the editor then fails to recognize, deleting it from the buffer).
            Value::String(s) => Value::String(
                holon_api::content_canonical::canonicalize_stored_content(s, is_source),
            ),
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
                        "SqlOperationProvider: edge field '{}' on '{}' must be Value::Array, got \
                         {:?}",
                        key, self.entity_name, other
                    ),
                };
                let ids: Vec<String> = arr
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => panic!(
                            "SqlOperationProvider: edge field '{}' items must be Value::String, \
                             got {:?}",
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

    /// Whether a `block_raw` row with `id` currently exists. Used to attribute
    /// a create-time FK failure accurately: the parent FK and the junction
    /// source FKs both surface the same opaque message, so "parent not
    /// found" must be proven, not assumed.
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

    /// A block's real parent id, or `None` when it has no parent (NULL column
    /// or the `sentinel:no_parent` root — where seed pages legally live). Used
    /// by the `add_tag("Page")` nesting guard.
    async fn read_real_parent_id(&self, id: &str) -> Result<Option<String>> {
        let sql = format!(
            "SELECT parent_id FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("read_real_parent_id({id}): {e}"))?;
        let pid = rows
            .into_iter()
            .next()
            .and_then(|mut r| r.remove("parent_id"));
        Ok(match pid {
            Some(Value::String(s)) if !s.is_empty() && s != EntityUri::no_parent().as_str() => {
                Some(s)
            }
            _ => None,
        })
    }

    /// Whether `id` carries the `Page` tag (mirrors the `tag='Page'` join
    /// precedent used by `resolve_page_name`).
    pub async fn block_is_page(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM block_tags WHERE block_id = '{}' AND tag = '{}' LIMIT 1",
            id.replace('\'', "''"),
            PAGE_TAG,
        );
        Ok(!self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("block_is_page({id}): {e}"))?
            .is_empty())
    }

    /// Whether `id` has any DIRECT child carrying the `Page` tag — the
    /// `remove_tag("Page")` guard: unmarking a page with page children would
    /// leave those children as pages under a non-page block.
    async fn has_page_child(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM block_raw c JOIN block_tags t ON t.block_id = c.id AND t.tag = '{}' \
             WHERE c.parent_id = '{}' LIMIT 1",
            PAGE_TAG,
            id.replace('\'', "''"),
        );
        Ok(!self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("has_page_child({id}): {e}"))?
            .is_empty())
    }

    /// The inverse [`Operation`] of an element-wise tag op: `remove_tag` undoes
    /// `add_tag` and vice-versa, same `{id, tag}` params.
    fn tag_inverse(&self, inverse_op: &str, id: &str, tag: &str) -> Operation {
        Operation::from_params(
            EntityName::new(&self.entity_name),
            inverse_op,
            inverse_op,
            [
                ("id".to_string(), Value::String(id.to_string())),
                ("tag".to_string(), Value::String(tag.to_string())),
            ],
        )
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

    /// Execute a prepared operation: run its SQL statements, rows before edges
    /// so a junction never precedes its referenced `block_raw` row.
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
        // Without this, CacheEventSubscriber fails to deserialize: "missing field
        // created_at".
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
    /// Async because it reads the existing row to merge `properties` JSON and
    /// to run the per-column diff guard that suppresses no-op UPDATEs.
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
                "[CUSTOMPROP-TRACE prepare_update] id={id} custom_keys={:?} extra_props={:?} \
                 sql_fields_keys={:?}",
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
                        "prepare_delete: parent cycle detected while cascading delete of '{id}' — \
                         block '{child}' is its own ancestor (corrupt block tree)"
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

        // block_links (links increment 2) has NO FK (soft targets), so clean
        // up explicitly for every cascaded id: drop the deleted block's own
        // link rows, and un-resolve inbound links that pointed at it — the
        // target is gone, those links are dangling again (re-resolvable when
        // a matching page reappears).
        let mut edge_statements = Vec::new();
        if self.entity_name == "block" {
            for gone in all_ids.iter().chain(std::iter::once(&id.to_string())) {
                let g = gone.replace('\'', "''");
                edge_statements.push(format!(
                    "DELETE FROM block_links WHERE source_block_id = '{g}'"
                ));
                edge_statements.push(format!(
                    "UPDATE block_links SET resolved_id = NULL WHERE resolved_id = '{g}'"
                ));
            }
        }

        Ok(PreparedOp {
            row_statements,
            edge_statements,
        })
    }

    // ─── block_links junction (links increment 2) ────────────────────────
    //
    // Rows derive from the block's `marks` Link spans at THIS write boundary,
    // in the same transaction as the block row and the other junctions.
    // Soft targets: dangling (`resolved_id` NULL) is representable, no FK —
    // pages are created lazily, never as placeholders.

    /// Statement set replacing `source_id`'s `block_links` rows with the
    /// links derived from the `marks` param (JSON string; Null = no marks).
    /// Name-form (`kind='page'`) targets are resolved NOW against existing
    /// Page-tagged blocks; unresolved targets stay dangling.
    async fn block_link_statements(
        &self,
        source_id: &str,
        marks_value: &Value,
    ) -> Result<Vec<String>> {
        let marks: Vec<holon_api::MarkSpan> = match marks_value {
            Value::Null => Vec::new(),
            Value::Json(s) | Value::String(s) => {
                if s.is_empty() {
                    Vec::new()
                } else {
                    holon_api::marks_from_json(s).map_err(|e| {
                        format!("block_links: 'marks' param holds invalid JSON {s:?}: {e}")
                    })?
                }
            }
            other => {
                return Err(format!(
                    "block_links: 'marks' param must be a JSON string or Null, got {other:?}"
                )
                .into());
            }
        };
        let sid = source_id.replace('\'', "''");
        let mut stmts = vec![format!(
            "DELETE FROM block_links WHERE source_block_id = '{sid}'"
        )];
        for link in holon_api::derive_block_links(&marks) {
            let resolved = match &link.resolved {
                Some(id) => Some(id.as_str().to_string()),
                None => self.resolve_page_name(&link.target).await?,
            };
            let resolved_sql = match resolved {
                Some(r) => format!("'{}'", r.replace('\'', "''")),
                None => "NULL".to_string(),
            };
            stmts.push(format!(
                "INSERT OR REPLACE INTO block_links (source_block_id, target, kind, resolved_id) \
                 VALUES ('{sid}', '{}', '{}', {resolved_sql})",
                link.target.replace('\'', "''"),
                link.kind.as_str(),
            ));
        }
        Ok(stmts)
    }

    /// Resolve a wiki-name target (possibly a `parent/leaf` chain) to an
    /// existing Page-tagged block. Suffix semantics: the LEAF names the page;
    /// a preceding segment is a disambiguation HINT preferring candidates
    /// whose parent block carries that name. Deterministic (ties by id).
    /// `None` = no matching page yet — the link stays dangling until
    /// `page_reresolve_statements` fires on a matching Page write.
    async fn resolve_page_name(&self, target: &str) -> Result<Option<String>> {
        let mut segs = target.rsplit('/');
        let leaf = segs.next().unwrap_or(target).trim();
        if leaf.is_empty() {
            return Ok(None);
        }
        let parent_hint = segs.next().map(|s| s.trim().to_string());
        let leaf_sql = leaf.replace('\'', "''");
        let sql = match &parent_hint {
            Some(hint) => format!(
                "SELECT b.id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND t.tag = \
                 'Page' LEFT JOIN block_raw p ON p.id = b.parent_id WHERE b.content = \
                 '{leaf_sql}' ORDER BY CASE WHEN p.content = '{}' THEN 0 ELSE 1 END, b.id LIMIT 1",
                hint.replace('\'', "''"),
            ),
            None => format!(
                "SELECT b.id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND t.tag = \
                 'Page' WHERE b.content = '{leaf_sql}' ORDER BY b.id LIMIT 1"
            ),
        };
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("block_links page resolution query failed: {e}"))?;
        Ok(rows.into_iter().next().and_then(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        }))
    }

    /// The origin's DIRECT children in the SAME order the tree presents them
    /// (`sort_key`, id as the stable tiebreak). The block→page transform
    /// re-homes each under the new page; the order is captured so the
    /// forward `move_block`s (and their exact inverses) reproduce sibling
    /// order on both the transform and its undo.
    async fn read_ordered_children(&self, parent_id: &str) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT id FROM {} WHERE parent_id = '{}' ORDER BY sort_key, id",
            self.table_name,
            parent_id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("read_ordered_children({parent_id}): {e}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .collect())
    }

    /// The block's stored `depth` (tree level; a top-level page is 0). A
    /// missing or NULL depth reads as 0 — the root level — rather than
    /// failing, matching how a freshly-seeded page with no explicit depth
    /// behaves.
    async fn read_block_depth(&self, id: &str) -> Result<i64> {
        match self.read_field_old_value(id, "depth").await? {
            Value::Integer(d) => Ok(d),
            Value::Null => Ok(0),
            other => {
                Err(format!("read_block_depth({id}): depth is not an integer: {other:?}").into())
            }
        }
    }

    /// The nearest ancestor of `id` that carries the `Page` tag, walking the
    /// real parent chain upward. `None` when the chain reaches the root with no
    /// page — the block→page transform then defaults to the vault root.
    async fn nearest_page_ancestor(&self, id: &str) -> Result<Option<String>> {
        let mut cursor = self.read_real_parent_id(id).await?;
        let mut guard = 0usize;
        while let Some(pid) = cursor {
            guard += 1;
            if guard > 1024 {
                return Err(format!("nearest_page_ancestor({id}): parent chain too deep").into());
            }
            if self.block_is_page(&pid).await? {
                return Ok(Some(pid));
            }
            cursor = self.read_real_parent_id(&pid).await?;
        }
        Ok(None)
    }

    /// The `/`-joined page path (root→leaf) of an existing page, reconstructed
    /// by walking its page-ancestor chain and collecting each page's
    /// `content` title. Used to seed the transform's DEFAULT destination
    /// from the origin's nearest page ancestor, so `PageId::for_path`
    /// computes the new page's id against the same path string the
    /// destination page was minted with.
    async fn page_path_of(&self, page_id: &str) -> Result<String> {
        let mut segments: Vec<String> = Vec::new();
        let mut cursor = Some(page_id.to_string());
        let mut guard = 0usize;
        while let Some(id) = cursor {
            guard += 1;
            if guard > 1024 {
                return Err(format!("page_path_of({page_id}): parent chain too deep").into());
            }
            if !self.block_is_page(&id).await? {
                break;
            }
            let title = match self.read_field_old_value(&id, "content").await? {
                Value::String(s) => s.trim().to_string(),
                _ => break,
            };
            segments.push(title);
            cursor = self.read_real_parent_id(&id).await?;
        }
        segments.reverse();
        Ok(segments.join("/"))
    }

    /// Resolve the destination page chain for a block→page transform WITHOUT
    /// writing anything. Walks the `/`-joined `destination_path` segment by
    /// segment: an existing `Page` block is reused; a missing one is recorded
    /// (with the deterministic id `PageId::for_path` will assign) so the engine
    /// can create it as an invertible `create`. Returns the leaf parent id plus
    /// the ordered list of pages the engine must mint first.
    ///
    /// An empty `destination_path` targets the vault root
    /// (`sentinel:no_parent`).
    /// Returns `(destination_parent_id, destination_parent_depth, missing)`.
    /// A top-level page has `depth = 0`, so the vault-root base depth is `-1`
    /// (its first child page is `-1 + 1 = 0`). Each created segment and the
    /// final new page thus get a tree-consistent `depth = parent.depth + 1`.
    async fn resolve_destination_chain(
        &self,
        destination_path: &str,
    ) -> Result<(
        String,
        i64,
        Vec<crate::core::block_to_page_plan::PlanSegment>,
    )> {
        use crate::core::block_to_page_plan::PlanSegment;

        let trimmed_path = destination_path.trim();
        if trimmed_path.is_empty() {
            return Ok((EntityUri::no_parent().as_str().to_string(), -1, Vec::new()));
        }

        let mut parent_id = EntityUri::no_parent().as_str().to_string();
        let mut parent_depth: i64 = -1;
        let mut accumulated = String::new();
        let mut missing: Vec<PlanSegment> = Vec::new();

        for seg in trimmed_path.split('/') {
            let name = seg.trim();
            if name.is_empty() {
                return Err(format!(
                    "block_to_page_plan: empty segment in destination_path '{destination_path}'"
                )
                .into());
            }
            let seg_path = if accumulated.is_empty() {
                name.to_string()
            } else {
                format!("{accumulated}/{name}")
            };
            let hint = if accumulated.is_empty() {
                name.to_string()
            } else {
                format!("{accumulated}/{name}")
            };
            match self.resolve_page_name(&hint).await? {
                Some(existing) => {
                    parent_depth = self.read_block_depth(&existing).await?;
                    parent_id = existing;
                }
                None => {
                    let id = holon_api::link_parser::PageId::for_path(&seg_path)?
                        .as_str()
                        .to_string();
                    let depth = parent_depth + 1;
                    missing.push(PlanSegment {
                        id: id.clone(),
                        name: name.to_string(),
                        parent_id: parent_id.clone(),
                        depth,
                    });
                    parent_id = id;
                    parent_depth = depth;
                }
            }
            accumulated = seg_path;
        }
        Ok((parent_id, parent_depth, missing))
    }

    /// The dangling→resolved trigger: when a Page-tagged block is written,
    /// dangling name links it satisfies resolve to it (leaf-suffix match —
    /// target equal to the page name or a chain ending in `/<name>`). This is
    /// the cheapest correct re-resolution point: dangling rows are touched
    /// exactly when a page that could satisfy them appears.
    fn page_reresolve_statements(id: &str, name: &str) -> Vec<String> {
        if name.is_empty() {
            return Vec::new();
        }
        let idq = id.replace('\'', "''");
        let eq = name.replace('\'', "''");
        let like = name
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('\'', "''");
        vec![format!(
            "UPDATE block_links SET resolved_id = '{idq}' WHERE resolved_id IS NULL AND kind = \
             'page' AND (target = '{eq}' OR target LIKE '%/{like}' ESCAPE '\\')"
        )]
    }

    /// Capture the `block_links` rows currently resolved to `resolved_id`, as
    /// the row objects `restore_link_resolution` replays to undo a
    /// `rewrite_link_resolution`. Each object carries the full PRIMARY KEY
    /// (`source_block_id`, `target`, `kind`) plus the prior `resolved_id` —
    /// here always equal to the queried id, but captured verbatim so the
    /// restore is self-describing (no reliance on the forward `from`).
    async fn capture_links_resolved_to(&self, resolved_id: &str) -> Result<Vec<Value>> {
        let sql = format!(
            "SELECT source_block_id, target, kind, resolved_id FROM block_links WHERE \
             resolved_id = '{rid}'",
            rid = resolved_id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("capture_links_resolved_to({resolved_id}): {e}"))?;
        Ok(rows
            .into_iter()
            .map(|row| Value::Object(row.into_iter().map(|(k, v)| (k.to_string(), v)).collect()))
            .collect())
    }

    /// Build the `UPDATE block_links` statements that restore each captured
    /// row's prior `resolved_id`, keyed on the junction PRIMARY KEY. A
    /// `Null` `resolved_id` restores to `NULL` (dangling). Fails loud on a
    /// malformed captured row (missing/typed-wrong key) rather than
    /// silently skipping it.
    fn restore_links_statements(rows: &[Value]) -> Result<Vec<String>> {
        let mut stmts = Vec::with_capacity(rows.len());
        for row in rows {
            let obj = match row {
                Value::Object(o) => o,
                other => {
                    return Err(format!(
                        "restore_link_resolution: row is not an Object: {other:?}"
                    )
                    .into());
                }
            };
            let field = |name: &str| -> Result<String> {
                match obj.get(name) {
                    Some(v) => v.as_string().map(str::to_string).ok_or_else(|| {
                        format!("restore_link_resolution: '{name}' is not a string: {v:?}").into()
                    }),
                    None => Err(format!("restore_link_resolution: missing '{name}'").into()),
                }
            };
            let source = field("source_block_id")?;
            let target = field("target")?;
            let kind = field("kind")?;
            let set = match obj.get("resolved_id") {
                Some(Value::String(id)) => format!("'{}'", id.replace('\'', "''")),
                Some(Value::Null) | None => "NULL".to_string(),
                Some(other) => {
                    return Err(format!(
                        "restore_link_resolution: 'resolved_id' unexpected type: {other:?}"
                    )
                    .into());
                }
            };
            stmts.push(format!(
                "UPDATE block_links SET resolved_id = {set} WHERE source_block_id = '{s}' AND \
                 target = '{t}' AND kind = '{k}'",
                s = source.replace('\'', "''"),
                t = target.replace('\'', "''"),
                k = kind.replace('\'', "''"),
            ));
        }
        Ok(stmts)
    }

    // ─── block_redirects junction (merge_blocks) ─────────────────────────

    /// Statement set re-deriving `to_id`'s redirect rows from its
    /// `merged_from` property — the same replace-from-the-authoritative-field
    /// shape as [`Self::block_link_statements`]. A `Null` value (the undo of a
    /// merge, which `json_remove`s the property) yields just the DELETE, so
    /// undoing a merge retracts its redirect.
    fn block_redirect_statements(to_id: &str, merged_from: &Value) -> Result<Vec<String>> {
        let entries = merge_blocks_plan::parse_merged_from(merged_from)?;
        let toq = to_id.replace('\'', "''");
        let mut stmts = vec![format!("DELETE FROM block_redirects WHERE to_id = '{toq}'")];
        for (from_id, at) in entries {
            // A plain INSERT, not INSERT OR REPLACE: an id can only ever be
            // merged away once, the planner already refuses a second merge, and
            // the PRIMARY KEY is the last line of that promise. Silently
            // overwriting would let a re-derivation retarget a live redirect.
            stmts.push(format!(
                "INSERT INTO block_redirects (from_id, to_id, merged_at) VALUES ('{f}', \
                 '{toq}', {at})",
                f = from_id.replace('\'', "''"),
            ));
        }
        Ok(stmts)
    }

    /// The id `id` currently resolves to, following merge redirect chains. An
    /// id nobody merged away resolves to itself, so a caller can route a lookup
    /// MISS through this unconditionally. Fails loud on a cycle rather than
    /// looping — `merge_blocks` refuses to create one, so reaching it means the
    /// table was corrupted.
    pub async fn follow_redirects(&self, id: &str) -> Result<String> {
        let mut current = id.to_string();
        let mut seen = vec![current.clone()];
        loop {
            let sql = format!(
                "SELECT to_id FROM block_redirects WHERE from_id = '{}'",
                current.replace('\'', "''")
            );
            let rows = self
                .db_handle
                .query(&sql, HashMap::new())
                .await
                .map_err(|e| format!("follow_redirects({id}): {e}"))?;
            let next = match rows
                .first()
                .and_then(|r| r.get("to_id"))
                .and_then(|v| v.as_string())
            {
                Some(next) => next.to_string(),
                None => return Ok(current),
            };
            if seen.contains(&next) {
                return Err(format!(
                    "block_redirects holds a cycle reached from {id}: {seen:?} -> {next}"
                )
                .into());
            }
            seen.push(next.clone());
            current = next;
        }
    }

    /// Read one block's plan-relevant columns. `None` when the row is absent.
    async fn read_merge_side(&self, id: &str) -> Result<Option<(String, Value, i64)>> {
        let sql = format!(
            "SELECT content, properties, created_at FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("merge_blocks_plan: reading {id}: {e}"))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let content = row
            .get("content")
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        let properties = row.get("properties").cloned().unwrap_or(Value::Null);
        let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(Some((content, properties, created_at)))
    }

    /// `parent`'s direct children in sibling order, as plan children. A child
    /// whose `properties` carry an `ID` was authored in an org file, which wins
    /// a dedupe collapse over a minted id.
    async fn read_merge_children(&self, parent: &str) -> Result<Vec<merge_blocks_plan::PlanChild>> {
        let sql = format!(
            "SELECT id, content, properties, created_at FROM {} WHERE parent_id = '{}' ORDER BY \
             sort_key",
            self.table_name,
            parent.replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("merge_blocks_plan: reading children of {parent}: {e}"))?;
        let mut children = Vec::with_capacity(rows.len());
        for row in rows {
            let field = |name: &str| {
                row.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            children.push(merge_blocks_plan::PlanChild {
                id: field("id"),
                content: field("content"),
                authored: Self::properties_carry_authored_id(row.get("properties"))?,
                created_at: row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0),
            });
        }
        Ok(children)
    }

    /// A block's `properties` column as a map. The SQL read boundary
    /// (`normalize_known_json_columns`) has already parsed the JSON column into
    /// a [`Value::Object`] before any provider sees it, so that — not JSON text
    /// — is the shape to read. Absent/Null is an empty map; any other shape is
    /// a boundary violation and fails loud.
    fn properties_map(properties: Option<&Value>) -> Result<HashMap<String, Value>> {
        match properties {
            None | Some(Value::Null) => Ok(HashMap::new()),
            Some(Value::Object(map)) => Ok(map.clone()),
            Some(other) => Err(format!(
                "block properties must reach the merge planner as an object, got {other:?}"
            )
            .into()),
        }
    }

    /// Whether a block's properties carry an org-authored `:ID:`.
    fn properties_carry_authored_id(properties: Option<&Value>) -> Result<bool> {
        Ok(matches!(
            Self::properties_map(properties)?.get("ID"),
            Some(Value::String(_))
        ))
    }

    /// Read one key out of a block's properties, as `Value::Null` when the
    /// properties or the key are absent.
    fn property_from_blob(properties: &Value, key: &str) -> Result<Value> {
        Ok(Self::properties_map(Some(properties))?
            .get(key)
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The `donor` properties whose keys `holder` does not already carry —
    /// the merge adopts only these, so the canonical wins every conflict.
    fn properties_absent_from(donor: &Value, holder: &Value) -> Result<Vec<(String, Value)>> {
        let map = Self::properties_map(Some(donor))?;
        let mut out = Vec::new();
        for key in map.keys() {
            // The donor's own merge provenance is NOT adopted: it names ids that
            // redirect to the DONOR, and the merge re-points them by chain.
            if key == merge_blocks_plan::MERGED_FROM_FIELD {
                continue;
            }
            // NEVER adopt the donor's identity. Copying its authored `:ID:` onto
            // the survivor makes write-back render `:ID: <merged-away id>` — which
            // re-creates the very split-root shape this operation exists to
            // repair. Internal underscore-prefixed keys (`_provenance`) are the
            // writer's own bookkeeping and are equally not the donor's to give.
            if key == "ID" || key.starts_with('_') {
                continue;
            }
            if !matches!(Self::property_from_blob(holder, key)?, Value::Null) {
                continue;
            }
            let value = Self::property_from_blob(donor, key)?;
            if !matches!(value, Value::Null) {
                out.push((key.clone(), value));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Every tag on `id`.
    async fn read_block_tags(&self, id: &str) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT tag FROM block_tags WHERE block_id = '{}' ORDER BY tag",
            id.replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("merge_blocks_plan: reading tags of {id}: {e}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.get("tag").and_then(|v| v.as_string()).map(str::to_string))
            .collect())
    }

    /// Whether `ancestor` sits on `descendant`'s parent chain. Walked one hop
    /// at a time rather than as a recursive CTE: the self-parented
    /// `sentinel:no_parent` root makes the CTE form fragile, and the chain is
    /// short. Fails loud on a parent cycle instead of spinning.
    async fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let mut current = descendant.to_string();
        let mut seen = vec![current.clone()];
        loop {
            let sql = format!(
                "SELECT parent_id FROM {} WHERE id = '{}'",
                self.table_name,
                current.replace('\'', "''")
            );
            let rows = self
                .db_handle
                .query(&sql, HashMap::new())
                .await
                .map_err(|e| {
                    format!("merge_blocks_plan: ancestor walk {ancestor}/{descendant}: {e}")
                })?;
            let Some(parent) = rows
                .first()
                .and_then(|r| r.get("parent_id"))
                .and_then(|v| v.as_string())
                .map(str::to_string)
            else {
                return Ok(false);
            };
            if parent == ancestor {
                return Ok(true);
            }
            // The root sentinel is its own parent, which terminates the walk.
            if parent == current {
                return Ok(false);
            }
            if seen.contains(&parent) {
                return Err(format!(
                    "block parent chain from {descendant} holds a cycle: {seen:?} -> {parent}"
                )
                .into());
            }
            seen.push(parent.clone());
            current = parent;
        }
    }

    /// Whether a document file is bound to `id` — merging such a block away
    /// would strand its file, which Inc 1 refuses rather than guesses at.
    async fn has_file_binding(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT id FROM file WHERE document_id = '{}'",
            id.replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("merge_blocks_plan: file binding of {id}: {e}"))?;
        Ok(!rows.is_empty())
    }

    /// True when a block-write param set tags the block as a Page.
    fn params_tag_page(params: &StorageEntity) -> bool {
        matches!(params.get("tags"), Some(Value::Array(tags))
            if tags.iter().any(|t| t.as_string() == Some("Page")))
    }

    /// Read the current value of `field` for row `id` so an inverse
    /// [`Operation`] can restore it. Known SQL columns are read directly;
    /// everything else is a `properties` JSON entry. A missing row or a
    /// null/absent field both read back as [`Value::Null`] — the sentinel the
    /// inverse `set_field` uses to REMOVE a property (`json_remove`).
    async fn read_field_old_value(&self, id: &str, field: &str) -> Result<Value> {
        let sql = if self.known_columns.contains(field) {
            format!(
                "SELECT {col} AS v FROM {table} WHERE id = '{id}'",
                col = Self::quote_identifier(field),
                table = self.table_name,
                id = id.replace('\'', "''"),
            )
        } else {
            format!(
                "SELECT json_extract(properties, '$.{field}') AS v FROM {table} WHERE id = '{id}'",
                field = field.replace('\'', "''"),
                table = self.table_name,
                id = id.replace('\'', "''"),
            )
        };
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("read_field_old_value({field}): {e}"))?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|mut r| r.remove("v"))
            .unwrap_or(Value::Null))
    }

    /// Build the inverse `set_field` [`Operation`] that restores `field` to
    /// `old_value` on row `id`. The param shape ({id, field, value}) matches
    /// the forward op so the undo stack's word-boundary coalescer (which
    /// inspects `set_field` op params) recognizes single-character text
    /// edits.
    fn set_field_inverse(&self, id: &str, field: &str, old_value: Value) -> Operation {
        Operation::from_params(
            EntityName::new(&self.entity_name),
            "set_field",
            "set_field",
            [
                ("id".to_string(), Value::String(id.to_string())),
                ("field".to_string(), Value::String(field.to_string())),
                ("value".to_string(), old_value),
            ],
        )
    }

    /// Build the `{"text","marks"}` Object value a rich-content inverse
    /// restores — the SQL mirror of Loro's `rich_content_restore_value`. Text
    /// and marks travel as ONE atomic `content` write, so undo restores the
    /// exact prior (content, marks) pair. Crucially, the dispatcher only splits
    /// off a derived `marks` follow-up for a *String* content value (it reads
    /// `value.as_string()`), so an Object value NEVER triggers the re-derive
    /// that would otherwise clobber the restored marks.
    fn rich_content_value(text: &str, marks: &Value) -> Value {
        let marks_json = match marks {
            Value::String(s) | Value::Json(s) => s.clone(),
            _ => "[]".to_string(),
        };
        let mut obj = std::collections::HashMap::new();
        obj.insert("text".to_string(), Value::String(text.to_string()));
        obj.insert("marks".to_string(), Value::String(marks_json));
        Value::Object(obj)
    }

    /// True when a `marks` column value carries at least one span (a non-empty,
    /// non-`[]` JSON array). Drives the choice between a rich Object inverse
    /// (restore both content AND marks) and a plain String content inverse.
    fn marks_non_empty(marks: &Value) -> bool {
        match marks {
            Value::String(s) | Value::Json(s) => !s.is_empty() && s != "[]" && s != "null",
            _ => false,
        }
    }

    /// Rich `content` write: text AND marks as ONE atomic value (the SQL mirror
    /// of the Loro `content=Object` path). Writes both columns, re-derives the
    /// `block_links` junction from the marks, and returns an inverse that
    /// restores the exact prior (content, marks) pair. Reached only via
    /// undo/redo replay of a rich inverse — interactive SqlOnly content edits
    /// arrive as String values and are split into a content write plus a
    /// dispatcher-derived `marks` follow-up.
    async fn set_field_content_rich(
        &self,
        id: &str,
        obj: &std::collections::HashMap<String, Value>,
    ) -> Result<OperationResult> {
        let text = obj
            .get("text")
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                format!("set_field(content): Object value missing string 'text': {obj:?}")
            })?
            .to_string();
        let marks_val = obj.get("marks").cloned().unwrap_or(Value::Null);
        match &marks_val {
            Value::String(_) | Value::Json(_) | Value::Null => {}
            other => {
                return Err(format!(
                    "set_field(content): Object 'marks' must be a JSON string or Null, got {other:?}"
                )
                .into());
            }
        }

        // Trim per content_type (source blocks preserve first-line whitespace).
        let ct_sql = format!(
            "SELECT content_type FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''")
        );
        let is_source = self
            .db_handle
            .query(&ct_sql, HashMap::new())
            .await
            .map_err(|e| format!("set_field(content rich) content_type lookup: {e}"))?
            .into_iter()
            .next()
            .and_then(|row| {
                row.get("content_type")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .is_some_and(|s| s == "source");
        let trimmed = match Self::trimmed_content(&Value::String(text), is_source) {
            Value::String(s) => s,
            other => unreachable!("trimmed_content(String) yields String, got {other:?}"),
        };

        // Capture prior (content, marks) BEFORE the write — the marks column
        // still holds the prior set here (the interactive path's `marks`
        // follow-up runs AFTER this returns), so this is the true predecessor.
        let prior_content = self.read_field_old_value(id, "content").await?;
        let prior_marks = self.read_field_old_value(id, "marks").await?;

        let content_sql = Self::value_to_sql(&Value::String(trimmed.clone()));
        let marks_sql = match &marks_val {
            Value::String(s) | Value::Json(s) if !s.is_empty() && s != "[]" => {
                Self::value_to_sql(&Value::String(s.clone()))
            }
            _ => "NULL".to_string(),
        };
        let sql = format!(
            "UPDATE {} SET {} = {}, {} = {} WHERE id = '{}'",
            self.table_name,
            Self::quote_identifier("content"),
            content_sql,
            Self::quote_identifier("marks"),
            marks_sql,
            id.replace('\'', "''"),
        );
        self.db_handle
            .execute(&sql, vec![])
            .await
            .map_err(|e| format!("set_field(content rich) UPDATE failed: {e}"))?;

        // Re-derive the `block_links` junction from the restored marks (the
        // String path gets this from its separate `marks` follow-up; the Object
        // path owns it because no follow-up fires).
        if self.entity_name == "block" {
            let stmts: Vec<(String, Vec<turso::Value>)> = self
                .block_link_statements(id, &marks_val)
                .await?
                .into_iter()
                .map(|s| (s, vec![]))
                .collect();
            self.db_handle
                .transaction(stmts)
                .await
                .map_err(|e| format!("set_field(content rich) block_links update failed: {e}"))?;
        }

        // Inverse restores the prior pair: rich Object when the predecessor
        // carried marks (so undo↔redo across this write is symmetric), else a
        // plain String content restore.
        let prior_text = match &prior_content {
            Value::String(s) => s.clone(),
            _ => String::new(),
        };
        let inverse = if Self::marks_non_empty(&prior_marks) {
            self.set_field_inverse(
                id,
                "content",
                Self::rich_content_value(&prior_text, &prior_marks),
            )
        } else {
            self.set_field_inverse(id, "content", Value::String(prior_text))
        };

        // `content` is a projected column, so arm the stale-guard when the text
        // actually changed; a marks-only restore (text unchanged) leaves the
        // column untouched and reports no delta (single-writer safe).
        let new_content = Value::String(trimmed);
        let changes = if prior_content != new_content {
            vec![FieldDelta::new(
                id.to_string(),
                "content",
                prior_content,
                new_content,
            )]
        } else {
            Vec::new()
        };
        Ok(OperationResult::new(changes, inverse))
    }

    /// Capture the full `block_raw` row for `id` as create-op params (columns
    /// verbatim, minus the CDC-provenance sentinel `_change_origin`, which the
    /// writer stamps fresh). Returns `None` when the row is absent. Used to
    /// build the identity-preserving inverse of a leaf `delete`.
    async fn capture_row(&self, id: &str) -> Result<Option<StorageEntity>> {
        let sql = format!(
            "SELECT * FROM {table} WHERE id = '{id}'",
            table = self.table_name,
            id = id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("capture_row({id}): {e}"))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let params: StorageEntity = row
            .into_iter()
            .filter(|(k, v)| k.as_ref() != "_change_origin" && !matches!(v, Value::Null))
            .collect();
        Ok(Some(params))
    }

    /// Whether any row has `id` as its `parent_id` (i.e. the delete would
    /// cascade). A leaf (no children) delete is identity-invertible; a subtree
    /// delete is declared irreversible for now (see the `delete` arm).
    async fn has_children(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM {table} WHERE parent_id = '{id}' LIMIT 1",
            table = self.table_name,
            id = id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| format!("has_children({id}): {e}"))?;
        Ok(!rows.is_empty())
    }

    /// Read this entity's edge-field targets for `id` (one entry per declared
    /// edge field, as a `Value::Array` of target ids) so a delete's inverse
    /// `create` re-attaches them. Empty when the entity declares no edge
    /// fields.
    async fn capture_edges(&self, id: &str) -> Result<Vec<(String, Value)>> {
        let mut out = Vec::new();
        for descriptor in self.edge_fields.values() {
            let sql = format!(
                "SELECT {tc} AS t FROM {jt} WHERE {sc} = '{id}'",
                tc = Self::quote_identifier(&descriptor.target_col),
                jt = descriptor.join_table,
                sc = Self::quote_identifier(&descriptor.source_col),
                id = id.replace('\'', "''"),
            );
            let rows = self
                .db_handle
                .query(&sql, HashMap::new())
                .await
                .map_err(|e| format!("capture_edges({id}, {}): {e}", descriptor.field))?;
            let targets: Vec<Value> = rows
                .into_iter()
                .filter_map(|mut r| r.remove("t"))
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            if !targets.is_empty() {
                out.push((descriptor.field.clone(), Value::Array(targets)));
            }
        }
        Ok(out)
    }
}

impl SqlOperationProvider {
    /// Class (b) unique-random mint — PRIVATE to this impl (ADR 0029 D1c:
    /// `mint_unique` is not public trait surface). Preserves the pre-existing
    /// `{entity_name}:{uuid}` shape for every entity family; for the `block`
    /// family it equals the D1 owner `EntityUri::block_random`.
    fn mint_unique(&self) -> holon_api::identity_minting::MintedId {
        holon_api::identity_minting::MintedId::random_for_entity(&self.entity_name)
    }

    /// Read the current holder `content`/title of `id` from THIS authority's
    /// block table — the mode-specific half of D1b recognition. `Ok(None)` =
    /// the id is unheld. This is the single-row PK lookup that formerly
    /// lived inline in the create arm as the pre-SELECT collision guard; it
    /// now rides the minter trait's `mint` (single-source predicate via
    /// `recognize_derived_id`).
    async fn read_holder_title(
        &self,
        id: &EntityUri,
    ) -> std::result::Result<Option<String>, holon_api::identity_minting::BoxError> {
        let sql = format!(
            "SELECT content FROM {} WHERE id = '{}'",
            self.table_name,
            id.as_str().replace('\'', "''")
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| {
                format!(
                    "identity recognition: SELECT content for id {}: {e}",
                    id.as_str()
                )
            })?;
        Ok(rows
            .first()
            .and_then(|r| r.get("content"))
            .and_then(|v| v.as_string())
            .map(|s| s.to_string()))
    }
}

#[async_trait]
impl holon_api::identity_minting::IdentityMinting for SqlOperationProvider {
    async fn mint(
        &self,
        input: holon_api::identity_minting::IdentityInput,
    ) -> std::result::Result<
        holon_api::identity_minting::MintedId,
        holon_api::identity_minting::BoxError,
    > {
        use holon_api::identity_minting::IdentityInput;
        match input {
            IdentityInput::UniqueRandom => Ok(self.mint_unique()),
            IdentityInput::Carried { id, title } => {
                // Mode-specific store read of the derived id's current holder,
                // then the mode-INDEPENDENT single-source decision.
                let holder = self.read_holder_title(&id).await?;
                if matches!(
                    holon_api::identity_recognition::recognize_derived_id(
                        &id,
                        holder.as_deref(),
                        &title
                    ),
                    holon_api::identity_recognition::Recognition::UnnamedPlaceholder
                ) {
                    // Disclosed adoption, not a silent substitution: the holder
                    // carries no title, so this create names a placeholder that
                    // was standing in for this very id.
                    tracing::warn!(
                        id = %id,
                        requested_title = %title,
                        "adopting an UNNAMED placeholder row at a derived id — the create \
                         completes it with the requested title; no named holder is clobbered"
                    );
                }
                holon_api::identity_minting::bless_carried(id, holder.as_deref(), &title)
                    .map_err(|c| Box::new(c) as holon_api::identity_minting::BoxError)
            }
        }
    }
}

#[async_trait]
impl OperationProvider for SqlOperationProvider {
    /// This IS the Turso block-identity authority (ADR 0029 D1c). The active
    /// consolidator reaches the mint executor through this seam, mirroring
    /// `order_key_minter`.
    fn identity_minter(&self) -> Option<&dyn holon_api::identity_minting::IdentityMinting> {
        Some(self)
    }

    fn operations(&self) -> Vec<OperationDescriptor> {
        let mut ops = vec![
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
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "create".to_string(),
                display_name: "Create".to_string(),
                description: format!("Create a new {}", self.entity_short_name),
                id_column: "id".to_string(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
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
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
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
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
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
                id_column: "id".to_string(),
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "create_page_from_link".to_string(),
                display_name: "Create Page From Link".to_string(),
                description: "Create a page chain from a wiki-link target".to_string(),
                required_params: vec![OperationParam {
                    name: "target".to_string(),
                    type_hint: TypeHint::String,
                    description: "Wiki-link target (e.g. Projects/X)".to_string(),
                }],
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "rewrite_link_resolution".to_string(),
                display_name: "Rewrite Link Resolution".to_string(),
                description: "Re-point block_links resolved from one id to another".to_string(),
                required_params: vec![
                    OperationParam {
                        name: "from".to_string(),
                        type_hint: TypeHint::String,
                        description: "Current resolved_id to rewrite".to_string(),
                    },
                    OperationParam {
                        name: "to".to_string(),
                        type_hint: TypeHint::String,
                        description: "New resolved_id".to_string(),
                    },
                ],
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::IdentityOp,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "restore_link_resolution".to_string(),
                display_name: "Restore Link Resolution".to_string(),
                description: "Inverse of rewrite_link_resolution — restore captured \
                     resolved_ids"
                    .to_string(),
                required_params: vec![OperationParam {
                    name: "rows".to_string(),
                    type_hint: TypeHint::String,
                    description: "Captured block_links rows to restore".to_string(),
                }],
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::IdentityOp,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "block_to_page_plan".to_string(),
                display_name: "Block To Page Plan".to_string(),
                description: "Read-only planner for the convert_block_to_page compound".to_string(),
                required_params: vec![OperationParam {
                    name: "target".to_string(),
                    type_hint: TypeHint::String,
                    description: "Origin block id to convert".to_string(),
                }],
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                name: "merge_blocks_plan".to_string(),
                display_name: "Merge Blocks Plan".to_string(),
                description: "Read-only planner for the merge_blocks compound".to_string(),
                required_params: vec![
                    OperationParam {
                        name: "canonical".to_string(),
                        type_hint: TypeHint::String,
                        description: "The surviving block id".to_string(),
                    },
                    OperationParam {
                        name: "duplicate".to_string(),
                        type_hint: TypeHint::String,
                        description: "The block id folded away".to_string(),
                    },
                ],
                id_column: "id".to_string(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::Internal,
                },
                trigger: None,
                bound_params: Default::default(),
                precondition: None,
            },
        ];

        // `dismiss_advice` (ADR 0021/0022) is the SqlOnly-authority twin of the
        // Loro provider's op: it appends a lesson to the anchor's
        // `advice_suppressed` set. Only advertise it when this provider owns
        // that edge field (the `block` entity) — otherwise a non-block
        // SqlOperationProvider would falsely claim to dispatch it. This closes
        // BugFunnel row 26: in SqlOnly prod GPUI the block CRUD authority is
        // this provider, and the woven advice row's `dismiss_advice` op_button
        // was undispatchable ("No provider registered for entity: block").
        if self.edge_fields.contains_key("advice_suppressed") {
            let entity: EntityName = self.entity_name.clone().into();
            ops.push(holon_core::block_op_catalog::dismiss_advice_descriptor(
                &entity,
                &self.entity_short_name,
            ));
        }
        // Element-wise tag ops: only advertise when this provider owns the
        // `tags` edge field (the `block` entity), mirroring the row-26 lesson
        // that gated `dismiss_advice` on `advice_suppressed`.
        if self.edge_fields.contains_key("tags") {
            let entity: EntityName = self.entity_name.clone().into();
            ops.push(holon_core::block_op_catalog::add_tag_descriptor(
                &entity,
                &self.entity_short_name,
            ));
            ops.push(holon_core::block_op_catalog::remove_tag_descriptor(
                &entity,
                &self.entity_short_name,
            ));
        }
        ops
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

    /// This SQL backend owns readable block rows, so it answers the
    /// dispatcher's live-edit follow-up with ground truth: the stored
    /// stripped-label `content` and the `marks` column value. A missing row
    /// reads back as `Ok(None)` (unknown → the caller fails safe), matching
    /// the "never null on unknown" contract on the trait method.
    async fn read_block_content_marks(&self, id: &str) -> Result<Option<(String, Value)>> {
        let content = self.read_field_old_value(id, "content").await?;
        let marks = self.read_field_old_value(id, "marks").await?;
        match content {
            Value::String(s) => Ok(Some((s, marks))),
            Value::Null => Ok(None),
            other => Err(format!(
                "read_block_content_marks({id}): content column is not text: {other:?}"
            )
            .into()),
        }
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

                // Rich content write (text + marks as one Object) — the SQL
                // mirror of the Loro `content=Object` path. Reached via undo/redo
                // replay of a rich inverse; restores the exact (content, marks)
                // pair atomically. Interactive content edits arrive as Strings
                // and fall through to the split content + derived-marks path.
                if field == "content"
                    && let Value::Object(obj) = raw_value
                {
                    return self.set_field_content_rich(id, obj).await;
                }

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
                    // Capture the CURRENT edge targets BEFORE the replace so the
                    // inverse can restore them (edge writes are a whole-set
                    // replace, so the inverse is just `set_field(field, old_set)`).
                    let old_targets: Vec<Value> = self
                        .capture_edges(id)
                        .await?
                        .into_iter()
                        .find(|(f, _)| f == field)
                        .map(|(_, v)| match v {
                            Value::Array(items) => items,
                            _ => Vec::new(),
                        })
                        .unwrap_or_default();

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
                    // Edge targets live in a junction table the staleness reader
                    // (column-only) cannot fingerprint, so the precondition is
                    // empty (single-writer safe); the inverse is still a real
                    // whole-set restore.
                    let inverse = self.set_field_inverse(id, field, Value::Array(old_targets));
                    return Ok(OperationResult::new(Vec::new(), inverse));
                }

                let old_value = self.read_field_old_value(id, field).await?;

                // Editor echo-suppression ordering token. The gpui editor stamps
                // `write_seq` on each content keystroke (holon_api::write_seq) so
                // it can later drop stale/reordered CDC echoes of earlier
                // keystrokes. Persisted alongside the field write in the SAME
                // UPDATE so the row's content and its ordering token can never
                // diverge. Absent (all non-editor writers) → the column keeps its
                // prior value, so those writers' echoes carry `seq == last_local`
                // and still converge (they change `content`, not `write_seq`).
                let write_seq_pair = params
                    .get("write_seq")
                    .and_then(|v| v.as_i64())
                    .map(|seq| format!(", {} = {}", Self::quote_identifier("write_seq"), seq));

                let sql = if self.known_columns.contains(field) {
                    format!(
                        "UPDATE {} SET {} = {}{} WHERE id = '{}'",
                        self.table_name,
                        Self::quote_identifier(field),
                        sql_value,
                        write_seq_pair.as_deref().unwrap_or(""),
                        id.replace('\'', "''")
                    )
                } else if matches!(value, Value::Null) {
                    // Null means "remove this property" — use json_remove so we don't
                    // leave a {"key": null} entry in the JSON column. `task_state`
                    // removal also removes its `task_state_category` sidecar (the
                    // pair invariant `Block::set_task_state` establishes).
                    if field == "task_state" {
                        format!(
                            "UPDATE {} SET properties = json_remove(COALESCE(properties, '{{}}'), \
                             '$.task_state', '$.task_state_category') WHERE id = '{}'",
                            self.table_name,
                            id.replace('\'', "''")
                        )
                    } else {
                        format!(
                            "UPDATE {} SET properties = json_remove(COALESCE(properties, '{{}}'), \
                             '$.{}') WHERE id = '{}'",
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
                        "UPDATE {} SET properties = json_set(COALESCE(properties, '{{}}'), \
                         '$.task_state', {}, '$.task_state_category', '{}') WHERE id = '{}'",
                        self.table_name,
                        sql_value,
                        category,
                        id.replace('\'', "''")
                    )
                } else {
                    format!(
                        "UPDATE {} SET properties = json_set(COALESCE(properties, '{{}}'), \
                         '$.{}', {}) WHERE id = '{}'",
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

                // block_links junction (links increment 2): a marks write
                // replaces the source's derived link rows.
                if field == "marks" && self.entity_name == "block" {
                    let stmts: Vec<(String, Vec<turso::Value>)> = self
                        .block_link_statements(id, raw_value)
                        .await?
                        .into_iter()
                        .map(|s| (s, vec![]))
                        .collect();
                    self.db_handle
                        .transaction(stmts)
                        .await
                        .map_err(|e| format!("set_field(marks) block_links update failed: {e}"))?;
                }

                // block_redirects junction (merge_blocks): `merged_from` is the
                // replicated record of which ids this block absorbed, so its
                // redirect rows are re-derived here — including the undo, whose
                // property removal writes Null and clears them.
                if field == merge_blocks_plan::MERGED_FROM_FIELD && self.entity_name == "block" {
                    let stmts: Vec<(String, Vec<turso::Value>)> =
                        Self::block_redirect_statements(id, &value)?
                            .into_iter()
                            .map(|s| (s, vec![]))
                            .collect();
                    self.db_handle.transaction(stmts).await.map_err(|e| {
                        format!("set_field(merged_from) block_redirects update failed: {e}")
                    })?;
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

                // Inverse = set_field back to the pre-write value. Only COLUMN
                // fields carry a staleness fingerprint: the reader reads columns
                // only, so a `properties`-backed field (e.g. `task_state`) gets
                // an empty precondition (single-writer safe) but still a real
                // inverse. `Value::Null` old-value on a property drives the
                // inverse `json_remove`, restoring "absent" faithfully.
                let changes = if self.known_columns.contains(field) {
                    vec![FieldDelta::new(
                        id.to_string(),
                        field.to_string(),
                        old_value.clone(),
                        value.clone(),
                    )]
                } else {
                    Vec::new()
                };
                // A String content write over a block that ALREADY carried
                // marks must restore BOTH on undo: the dispatcher folds the
                // derived `marks` write into this same undoable step (no undo
                // entry of its own), so a content-only inverse would drop the
                // predecessor's marks (leaving nonsense-offset or absent spans).
                // Capture the prior marks (the column still holds them here —
                // the `marks` follow-up runs after this returns) and upgrade to
                // a rich Object inverse that restores the exact pair. A plain
                // predecessor (no marks) keeps the String inverse so the undo
                // stack's word-boundary coalescer still recognises typing.
                let inverse = if field == "content" {
                    let prior_marks = self.read_field_old_value(id, "marks").await?;
                    if Self::marks_non_empty(&prior_marks) {
                        let prior_text = old_value.as_string().unwrap_or_default().to_string();
                        self.set_field_inverse(
                            id,
                            "content",
                            Self::rich_content_value(&prior_text, &prior_marks),
                        )
                    } else {
                        self.set_field_inverse(id, field, old_value)
                    }
                } else {
                    self.set_field_inverse(id, field, old_value)
                };
                Ok(OperationResult::new(changes, inverse))
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
                // Identity is minted / recognized through the single authority
                // (ADR 0029 D1c) — the active consolidator's `identity_minter`,
                // not inlined here. A create without an `id` mints a fresh
                // unique-random one (the interactive / Rhai hot path); a SUPPLIED
                // id is a caller-DERIVED value (e.g. a page's `PageId::for_path`)
                // that must be RECOGNIZED against its current holder before it can
                // `INSERT ... ON CONFLICT(id) DO UPDATE`-land, so a rename (id
                // preserved, title changed) is never silently clobbered. The
                // former inline pre-SELECT collision guard is SUBSUMED into
                // `mint`, which uses the single-source `recognize_derived_id`
                // predicate (D1b interim fail-loud). Content-gated exactly as
                // before: with no title there is nothing to recognize.
                let minter = self.identity_minter().ok_or_else(
                    || -> Box<dyn std::error::Error + Send + Sync> {
                        "SqlOnly create requires an IdentityMinting seam (the Turso mint authority)"
                            .into()
                    },
                )?;
                let create_id: holon_api::identity_minting::CreateId =
                    match params.get("id").and_then(|v| v.as_string()) {
                        Some(existing) => {
                            let carried = holon_api::identity_minting::CarriedId::from_stored(
                                // ALLOW(entity_uri_from_raw): id is a validated create param
                                EntityUri::from_raw(existing),
                            );
                            if let Some(content) = params.get("content").and_then(|v| v.as_string())
                            {
                                // Recognize the carried id against its store
                                // holder; Err(IdentityCollision) on a rename
                                // clobber. The blessed MintedId is discarded — the
                                // CarriedId witness is the create id.
                                let content = content.to_string();
                                let carried_id = carried.as_entity_uri().clone();
                                minter
                                    .mint(holon_api::identity_minting::IdentityInput::carried(
                                        carried_id, content,
                                    ))
                                    .await?;
                            }
                            holon_api::identity_minting::CreateId::Carried(carried)
                        }
                        None => holon_api::identity_minting::CreateId::Minted(
                            minter
                                .mint(holon_api::identity_minting::IdentityInput::UniqueRandom)
                                .await?,
                        ),
                    };
                let id = create_id.as_str().to_string();
                params.insert("id".into(), Value::String(id.clone()));
                let prepared = self.prepare_create(&params);
                // block_links junction (links increment 2): derived from the
                // marks param, written in the SAME transaction as the row +
                // edge junctions. A Page-tagged create also re-resolves
                // dangling name links it satisfies.
                let mut link_statements: Vec<String> = Vec::new();
                if self.entity_name == "block" {
                    if let Some(marks) = params.get("marks") {
                        link_statements.extend(self.block_link_statements(&id, marks).await?);
                    }
                    if Self::params_tag_page(&params)
                        && let Some(content) = params.get("content").and_then(|v| v.as_string())
                    {
                        link_statements.extend(Self::page_reresolve_statements(&id, content));
                    }
                    if let Some(merged_from) = params.get(merge_blocks_plan::MERGED_FROM_FIELD) {
                        link_statements.extend(Self::block_redirect_statements(&id, merged_from)?);
                    }
                }
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
                        .chain(&link_statements)
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
                            "[SqlOp] SELECT after INSERT failed for '{}': {} — treating as not \
                             inserted",
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
                                .ok() // ALLOW(ok): id-collision lookup tolerance // ALLOW(fallback):
                                // pre-existing comment-only mention; not a real fallback.
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

                // Inverse of a genuine create = delete the minted/supplied id
                // (identity-preserving, ADR 0024). An IGNORED insert created
                // nothing, so undoing it must NOT delete the pre-existing row —
                // that path is declared irreversible. Forward fingerprint: the
                // `id` column is present post-create (absent → `Value::Null`
                // pre-create), so undo drops loudly if the row vanished under it.
                let mut result = if inserted {
                    let inverse = Operation::from_params(
                        EntityName::new(&self.entity_name),
                        "delete",
                        "delete",
                        [("id".to_string(), Value::String(id.clone()))],
                    );
                    OperationResult::new(
                        vec![FieldDelta::new(
                            id.clone(),
                            "id",
                            Value::Null,
                            Value::String(id.clone()),
                        )],
                        inverse,
                    )
                } else {
                    OperationResult::declared_irreversible(
                        Vec::new(),
                        "create: insert ignored (row already existed)",
                    )
                };
                result.response = response;
                Ok(result)
            }
            "update" => {
                let mut prepared = self.prepare_update(&params).await?;
                // block_links junction (links increment 2): a marks-carrying
                // update replaces the source's rows (idempotent DELETE+INSERT
                // — a same-value rewrite is a net-zero IVM delta); a
                // Page-tagged update re-resolves dangling links it satisfies.
                if self.entity_name == "block" {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .ok_or_else(|| "Missing 'id' parameter".to_string())?;
                    let mut link_statements: Vec<String> = Vec::new();
                    if let Some(marks) = params.get("marks") {
                        link_statements.extend(self.block_link_statements(id, marks).await?);
                    }
                    if Self::params_tag_page(&params)
                        && let Some(content) = params.get("content").and_then(|v| v.as_string())
                    {
                        link_statements.extend(Self::page_reresolve_statements(id, content));
                    }
                    if let Some(merged_from) = params.get(merge_blocks_plan::MERGED_FROM_FIELD) {
                        link_statements.extend(Self::block_redirect_statements(id, merged_from)?);
                    }
                    if !link_statements.is_empty() {
                        let mut p = prepared.unwrap_or(PreparedOp {
                            row_statements: Vec::new(),
                            edge_statements: Vec::new(),
                        });
                        p.edge_statements.extend(link_statements);
                        prepared = Some(p);
                    }
                }
                if let Some(prepared) = prepared {
                    // Atomicity: an edge-field write is DELETE-then-INSERT (per-block
                    // replace). In AUTOCOMMIT the Turso fork's deferred-FK check
                    // fails loud but does NOT roll back the partial DELETE, so a
                    // rejected INSERT would leave the junction half-cleared. Run row
                    // + edge statements in ONE transaction — mirrors the create path
                    // — so any failure rolls the whole update back instead of
                    // leaving a torn edge set. (`execute_prepared`'s per-statement
                    // autocommit did NOT give this guarantee.)
                    let stmts: Vec<(String, Vec<turso::Value>)> = prepared
                        .row_statements
                        .iter()
                        .chain(&prepared.edge_statements)
                        .map(|s| (s.clone(), vec![]))
                        .collect();
                    self.db_handle
                        .transaction(stmts)
                        .await
                        .map_err(|e| format!("Failed to execute update transaction: {}", e))?;
                }
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'id' parameter".to_string())?
                    .to_string();

                // Capture the FULL row + edges BEFORE deleting so a LEAF delete
                // is identity-invertible (inverse = `create` with the same id,
                // parent_id, sort_key, content, properties, edges — ADR 0024:
                // never delete+create where identity can be preserved). A delete
                // that CASCADES to descendants is declared irreversible for now:
                // faithfully resurrecting an ordered subtree is a Wave-2 concern.
                let captured = self.capture_row(&id).await?;
                let cascades = self.has_children(&id).await?;
                let inverse = match (&captured, cascades) {
                    (Some(row), false) => {
                        let mut create_params = row.clone();
                        for (field, targets) in self.capture_edges(&id).await? {
                            create_params.insert(field.into(), targets);
                        }
                        Some(Operation::from_params(
                            EntityName::new(&self.entity_name),
                            "create",
                            "create",
                            create_params.into_iter().map(|(k, v)| (k.to_string(), v)),
                        ))
                    }
                    _ => None,
                };

                let prepared = self.prepare_delete(&params).await?;
                self.execute_prepared(prepared).await?;

                // Forward fingerprint: the `id` column is absent post-delete
                // (present → its own value pre-delete), so an undo (`create`)
                // drops loudly if the row was resurrected under it.
                match inverse {
                    Some(create_op) => Ok(OperationResult::new(
                        vec![FieldDelta::new(
                            id.clone(),
                            "id",
                            Value::String(id.clone()),
                            Value::Null,
                        )],
                        create_op,
                    )),
                    None if captured.is_none() => Ok(OperationResult::declared_irreversible(
                        Vec::new(),
                        "delete: target row absent (nothing to resurrect)",
                    )),
                    None => Ok(OperationResult::declared_irreversible(
                        Vec::new(),
                        "delete: subtree capture not yet implemented",
                    )),
                }
            }
            "cycle_task_state" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'id' parameter".to_string())?
                    .to_string();

                let sql = format!(
                    "SELECT json_extract(properties, '$.task_state') as task_state FROM {} WHERE \
                     id = '{}'",
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
            "create_page_from_link" => {
                let target = params
                    .get("target")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'target' parameter".to_string())?
                    .to_string();

                if target.trim().is_empty() {
                    return Err("create_page_from_link: target must not be empty".into());
                }

                let segments: Vec<&str> = target.split('/').collect();
                let mut parent_id = "sentinel:no_parent".to_string();
                let mut accumulated = String::new();
                let mut leaf_id = String::new();

                for (i, seg) in segments.iter().enumerate() {
                    let trimmed = seg.trim();
                    if trimmed.is_empty() {
                        return Err(format!(
                            "create_page_from_link: empty segment in target '{target}'"
                        )
                        .into());
                    }

                    // Build the context-aware hint for resolve_page_name.
                    // For the first segment, just pass the segment name.
                    // For subsequent segments, include the accumulated prefix
                    // so resolve_page_name prefers pages under the right parent.
                    let hint = if i == 0 {
                        trimmed.to_string()
                    } else {
                        format!("{}/{}", accumulated, trimmed)
                    };

                    // Check if page already exists with the right parent.
                    let existing = self.resolve_page_name(&hint).await?;
                    match existing {
                        Some(page_id) => {
                            parent_id = page_id.clone();
                            leaf_id = page_id;
                            accumulated = if accumulated.is_empty() {
                                trimmed.to_string()
                            } else {
                                format!("{}/{}", accumulated, trimmed)
                            };
                            continue;
                        }
                        None => {
                            // Create the page at this level with a DETERMINISTIC
                            // id keyed on the accumulated path (root→this
                            // segment). Two peers that each lazily create the
                            // same page thus mint the SAME block id — the CRDT
                            // merge key — so a later merge yields ONE page, not a
                            // duplicate (inv-page-name-unique). Minted through the
                            // single `PageId` constructor so no write path can
                            // fall back to a random UUID.
                            let seg_path = if accumulated.is_empty() {
                                trimmed.to_string()
                            } else {
                                format!("{accumulated}/{trimmed}")
                            };
                            let id = holon_api::link_parser::PageId::for_path(&seg_path)?
                                .as_str()
                                .to_string();
                            let mut create_params: StorageEntity = HashMap::new();
                            create_params.insert("id".into(), Value::String(id.clone()));
                            create_params
                                .insert("content".into(), Value::String(trimmed.to_string()));
                            create_params
                                .insert("parent_id".into(), Value::String(parent_id.clone()));
                            create_params.insert(
                                "tags".into(),
                                Value::Array(vec![Value::String("Page".to_string())]),
                            );
                            self.execute_operation_with_origin(
                                entity_name,
                                "create",
                                create_params,
                                origin.clone(),
                            )
                            .await?;
                            parent_id = id.clone();
                            leaf_id = id;
                            accumulated = if accumulated.is_empty() {
                                trimmed.to_string()
                            } else {
                                format!("{}/{}", accumulated, trimmed)
                            };
                        }
                    }
                }

                // Heal any dangling name-links to this page chain.
                let link_stmts = Self::page_reresolve_statements(&leaf_id, &target);
                if !link_stmts.is_empty() {
                    let all_stmts: Vec<_> = link_stmts.into_iter().map(|s| (s, vec![])).collect();
                    self.db_handle.transaction(all_stmts).await.map_err(|e| {
                        format!("create_page_from_link: failed to heal dangling links: {e}")
                    })?;
                }

                // Grafting a new page into the tree is reversible via
                // `delete`, but we declare it irreversible because undo of
                // a multi-segment page-chain creation + link healing is
                // semantically noisy (undoing the chain creates dangling
                // links pointing nowhere). When ADR 0024 effect retraction
                // lands we can collapse this into a single undo entry.
                Ok(OperationResult::declared_irreversible(
                    Vec::new(),
                    "create_page_from_link — page-chain creation + link healing",
                )
                .with_response(Value::String(leaf_id)))
            }
            "rewrite_link_resolution" => {
                // Operation-level surface for the block→page transform's inbound
                // link re-pointing (transform doc §backlinks, Option B): rewrite
                // every `block_links` row currently resolved to `from` so it
                // resolves to `to` instead (origin → new page P). The EXACT
                // inverse captures the affected junction rows' prior
                // (source_block_id, target, kind, resolved_id) tuples up front
                // and restores each one — NOT a `to → from` swap, which would
                // wrongly recapture rows that already pointed at `to` before the
                // rewrite. Scoped to the transform's need; not a generic
                // SQL-undo framework.
                let from = params
                    .get("from")
                    .and_then(|v| v.as_string())
                    .ok_or("rewrite_link_resolution: missing 'from' parameter")?
                    .to_string();
                let to = params
                    .get("to")
                    .and_then(|v| v.as_string())
                    .ok_or("rewrite_link_resolution: missing 'to' parameter")?
                    .to_string();

                let prior_rows = self.capture_links_resolved_to(&from).await?;

                let fromq = from.replace('\'', "''");
                let toq = to.replace('\'', "''");
                let update = format!(
                    "UPDATE block_links SET resolved_id = '{toq}' WHERE resolved_id = '{fromq}'"
                );
                self.db_handle
                    .transaction(vec![(update, vec![])])
                    .await
                    .map_err(|e| format!("rewrite_link_resolution: {e}"))?;

                let inverse = Operation::from_params(
                    EntityName::new(&self.entity_name),
                    "restore_link_resolution",
                    "restore_link_resolution",
                    [("rows".to_string(), Value::Array(prior_rows))],
                );
                Ok(OperationResult::new(Vec::new(), inverse))
            }
            "restore_link_resolution" => {
                // Inverse-only surface: replay the captured junction rows,
                // restoring each PRIMARY KEY row's exact prior `resolved_id`.
                // Only ever dispatched as `rewrite_link_resolution`'s inverse
                // (undo); redo re-runs the forward `rewrite_link_resolution`, so
                // this op needs no inverse of its own — `declared_irreversible`
                // is the honest classification (and is ignored on inverse
                // replay).
                let rows = match params.get("rows") {
                    Some(Value::Array(rows)) => rows.clone(),
                    other => {
                        return Err(format!(
                            "restore_link_resolution: 'rows' must be an Array, got {other:?}"
                        )
                        .into());
                    }
                };
                let stmts = Self::restore_links_statements(&rows)?;
                if !stmts.is_empty() {
                    self.db_handle
                        .transaction(stmts.into_iter().map(|s| (s, vec![])).collect())
                        .await
                        .map_err(|e| format!("restore_link_resolution: {e}"))?;
                }
                Ok(OperationResult::declared_irreversible(
                    Vec::new(),
                    "restore_link_resolution — internal inverse of rewrite_link_resolution",
                ))
            }
            "merge_blocks_plan" => {
                // READ-ONLY planner for the engine-level `merge_blocks` compound.
                // Every precondition is checked HERE, before the engine dispatches
                // any constituent, so a refused merge leaves no partial state.
                use merge_blocks_plan::MergeBlocksPlan;

                let side = |name: &str| -> Result<String> {
                    Ok(params
                        .get(name)
                        .and_then(|v| v.as_string())
                        .ok_or_else(|| format!("merge_blocks_plan: missing '{name}' parameter"))?
                        .to_string())
                };
                let canonical_id = side("canonical")?;
                let duplicate_id = side("duplicate")?;

                if canonical_id == duplicate_id {
                    return Err(format!(
                        "merge_blocks: canonical and duplicate are the same block '{canonical_id}'"
                    )
                    .into());
                }
                let (canonical_content, canonical_properties, _) = self
                    .read_merge_side(&canonical_id)
                    .await?
                    .ok_or_else(|| format!("merge_blocks: canonical '{canonical_id}' not found"))?;
                let (duplicate_content, duplicate_properties, _) = self
                    .read_merge_side(&duplicate_id)
                    .await?
                    .ok_or_else(|| format!("merge_blocks: duplicate '{duplicate_id}' not found"))?;

                // Already merged away: the pair is settled, and re-merging would
                // append a second provenance entry for an id that no longer names
                // a live block.
                let already = self.follow_redirects(&duplicate_id).await?;
                if already != duplicate_id {
                    return Err(format!(
                        "merge_blocks: '{duplicate_id}' was already merged into '{already}'"
                    )
                    .into());
                }
                // Folding an ancestor into its own descendant would detach the
                // subtree between them.
                if self.is_ancestor_of(&duplicate_id, &canonical_id).await? {
                    return Err(format!(
                        "merge_blocks: '{duplicate_id}' is an ancestor of '{canonical_id}'; \
                         merging it away would detach the blocks between them"
                    )
                    .into());
                }
                if self.has_file_binding(&duplicate_id).await? {
                    return Err(format!(
                        "merge_blocks: '{duplicate_id}' is a document root with a live file \
                         binding; merging it away would strand that file (out of Inc 1 scope)"
                    )
                    .into());
                }

                // Deterministic post-merge order: the canonical's own children,
                // then the duplicate's, each keeping its relative order.
                let mut merged_children = self.read_merge_children(&canonical_id).await?;
                let canonical_child_count = merged_children.len() as i64;
                merged_children.extend(self.read_merge_children(&duplicate_id).await?);

                // Enrich each collapse with the reads the engine would otherwise
                // have to make mid-merge, when the tree is already half-moved.
                let mut dedupe_groups = Vec::new();
                for (keeper, loser_ids) in merge_blocks_plan::plan_dedupe(&merged_children) {
                    let keeper_last_child = self
                        .read_merge_children(&keeper)
                        .await?
                        .last()
                        .map(|c| c.id.clone());
                    let keeper_merged_from = {
                        let (_, props, _) =
                            self.read_merge_side(&keeper).await?.ok_or_else(|| {
                                format!("merge_blocks_plan: dedupe keeper '{keeper}' vanished")
                            })?;
                        let raw =
                            Self::property_from_blob(&props, merge_blocks_plan::MERGED_FROM_FIELD)?;
                        merge_blocks_plan::parse_merged_from(&raw)?
                    };
                    let mut losers = Vec::with_capacity(loser_ids.len());
                    for id in loser_ids {
                        let children = self
                            .read_merge_children(&id)
                            .await?
                            .into_iter()
                            .map(|c| c.id)
                            .collect();
                        losers.push(merge_blocks_plan::DedupeLoser { id, children });
                    }
                    dedupe_groups.push(merge_blocks_plan::DedupeGroup {
                        keeper,
                        keeper_last_child,
                        keeper_merged_from,
                        losers,
                    });
                }

                let existing_merged_from = {
                    let raw = Self::property_from_blob(
                        &canonical_properties,
                        merge_blocks_plan::MERGED_FROM_FIELD,
                    )?;
                    merge_blocks_plan::parse_merged_from(&raw)?
                };

                // Tags union, canonical first so a Page tag on EITHER side lands.
                let mut union_tags = self.read_block_tags(&canonical_id).await?;
                for tag in self.read_block_tags(&duplicate_id).await? {
                    if !union_tags.contains(&tag) {
                        union_tags.push(tag);
                    }
                }

                // Properties: canonical wins every conflict, so only the keys it
                // lacks are adopted from the duplicate.
                let adopted_properties =
                    Self::properties_absent_from(&duplicate_properties, &canonical_properties)?;

                let plan = MergeBlocksPlan {
                    canonical_id,
                    duplicate_id,
                    canonical_content,
                    duplicate_content,
                    merged_children,
                    canonical_child_count,
                    dedupe_groups,
                    existing_merged_from,
                    union_tags,
                    adopted_properties,
                    merged_at: self.clock.now_millis(),
                };
                Ok(OperationResult::declared_irreversible(
                    Vec::new(),
                    "merge_blocks_plan — read-only planner",
                )
                .with_response(plan.to_value()))
            }
            "block_to_page_plan" => {
                // READ-ONLY planner for the engine-level `convert_block_to_page`
                // compound (BlockToPageTransform Option B). Reads the origin's
                // content+marks, its ordered children, and resolves (but does NOT
                // create) the destination page chain — the engine executes each
                // constituent write as an ordinary dispatched op so it collects
                // the op-level inverse of each, assembling ONE composite
                // `UndoEntry`. This op writes nothing, so it is honestly
                // `declared_irreversible`.
                use crate::core::block_to_page_plan::BlockToPagePlan;

                let origin_id = params
                    .get("target")
                    .and_then(|v| v.as_string())
                    .ok_or("block_to_page_plan: missing 'target' parameter")?
                    .to_string();

                let (origin_content, origin_marks) = self
                    .read_block_content_marks(&origin_id)
                    .await?
                    .ok_or_else(|| {
                        format!("block_to_page_plan: origin block '{origin_id}' not found")
                    })?;

                // Fail loud: converting a page to a page is meaningless, and the
                // no-pages-under-non-pages interim rule means the ORIGIN's own
                // page-hood is not what we are establishing here (Option B mints a
                // NEW page; the origin stays a non-page link).
                if self.block_is_page(&origin_id).await? {
                    return Err(format!(
                        "block_to_page_plan: origin '{origin_id}' is already a page"
                    )
                    .into());
                }
                // The origin content is the page TITLE, and a page's title rides
                // its FILENAME on disk (the vault convention: zero `.org` files
                // carry `#+TITLE:`; the parser derives the title from the file
                // stem and the PBT model normalizes on it). A TRAILING `/` (the
                // slash-menu trigger still trailing at plan time, or a stray
                // separator) is retained in the DB title but DROPPED by path
                // normalization when the filename is formed — so an unsanitized
                // title reingests DIVERGENT from what convert wrote. Sanitize the
                // trailing `/` ONCE here (parse-don't-validate) so the page
                // title, its deterministic id, AND its filename all agree.
                // Interior `/` is left intact (namespace-meaningful; the
                // page-hierarchy ruling is PARKED).
                // Single-source the page-title sanitize (parse-don't-validate):
                // the convert planner, the reference model, and the recognition
                // step all funnel through `holon_api::sanitize_page_title`, so a
                // trailing-slash title can never be recognized raw on one side and
                // sanitized on the other (normalize_for_hash keeps '/').
                let raw_leaf = origin_content.trim();
                let page_title = holon_api::sanitize_page_title(&origin_content).ok_or_else(|| {
                    format!(
                        "block_to_page_plan: origin '{origin_id}' has empty content — a page needs \
                         a title"
                    )
                })?;
                if page_title != raw_leaf {
                    tracing::warn!(
                        origin = %origin_id,
                        raw = %raw_leaf,
                        sanitized = %page_title,
                        "block_to_page_plan: stripped trailing '/' from the page title so the \
                         title, its id, and its filename agree"
                    );
                }

                // Destination: an explicit `destination_path` (from the picker /
                // MCP) wins; otherwise PRE-SELECT the origin's nearest page
                // ancestor (transform doc sub-ruling 3). No page ancestor ⇒ the
                // vault root.
                let destination_path =
                    match params.get("destination_path").and_then(|v| v.as_string()) {
                        Some(p) => p.to_string(),
                        None => match self.nearest_page_ancestor(&origin_id).await? {
                            Some(anc) => self.page_path_of(&anc).await?,
                            None => String::new(),
                        },
                    };

                let (destination_parent_id, destination_parent_depth, missing_segments) =
                    self.resolve_destination_chain(&destination_path).await?;

                // The origin's content is the page TITLE — a single leaf
                // segment, NOT a `/`-separated path. Route it through
                // `for_page_under` so a `/` in the content (a legitimate title
                // like "buy milk/eggs", or the slash-menu trigger `/` still
                // trailing at plan time) is never split into page-path segments
                // (which would fail loud on an empty trailing segment, or
                // silently mint phantom hierarchy). Only the `destination_path`
                // is a real `/`-path, validated fail-loud inside `for_page_under`.
                let page_id =
                    holon_api::link_parser::PageId::for_page_under(&destination_path, &page_title)?
                        .as_str()
                        .to_string();
                let page_depth = destination_parent_depth + 1;

                let child_ids = self.read_ordered_children(&origin_id).await?;

                let plan = BlockToPagePlan {
                    origin_id,
                    origin_content: page_title,
                    origin_marks,
                    page_id,
                    page_depth,
                    destination_parent_id,
                    missing_segments,
                    child_ids,
                };
                Ok(OperationResult::declared_irreversible(
                    Vec::new(),
                    "block_to_page_plan is read-only",
                )
                .with_response(plan.to_value()))
            }
            "dismiss_advice" => {
                // Append the lesson to the anchor's `advice_suppressed` set
                // (ADR 0021/0022). A SINGLE per-row `INSERT OR IGNORE` against
                // the junction's PRIMARY KEY (anchor_id, lesson_id): idempotent
                // (a repeat dismiss is a PK-collision no-op) and inherently
                // conflict-free — there is no read-then-write, so two concurrent
                // dismissals cannot lose an update the way a whole-set
                // capture-then-replace would. Wrapped in `db_handle.transaction`
                // per the repo rule that block writes are transactional (even
                // though this junction's FK is IMMEDIATE, not the deferred-FK
                // autocommit-no-rollback hazard). This is intentionally NOT the
                // Loro provider's semantics: `LoroBlockOperations::dismiss_advice`
                // does a whole-array LWW replace over one meta key, so two
                // concurrent Loro dismissals of different lessons CAN clobber —
                // the SQL junction has no such limitation.
                let anchor_id = params
                    .get("anchor_id")
                    .and_then(|v| v.as_string())
                    .ok_or("dismiss_advice: missing 'anchor_id' parameter")?;
                let lesson_id = params
                    .get("lesson_id")
                    .and_then(|v| v.as_string())
                    .ok_or("dismiss_advice: missing 'lesson_id' parameter")?;
                let descriptor = self.edge_fields.get("advice_suppressed").ok_or(
                    "dismiss_advice: 'advice_suppressed' edge field is not registered on this \
                     provider",
                )?;

                let stmt = format!(
                    "INSERT OR IGNORE INTO {jt} ({sc}, {tc}) VALUES ('{anchor}', '{lesson}')",
                    jt = descriptor.join_table,
                    sc = Self::quote_identifier(&descriptor.source_col),
                    tc = Self::quote_identifier(&descriptor.target_col),
                    anchor = anchor_id.replace('\'', "''"),
                    lesson = lesson_id.replace('\'', "''"),
                );
                self.db_handle
                    .transaction(vec![(stmt, vec![])])
                    .await
                    .map_err(|e| format!("dismiss_advice: {e}"))?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "add_tag" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("add_tag: missing 'id' parameter")?;
                let tag = params
                    .get("tag")
                    .and_then(|v| v.as_string())
                    .ok_or("add_tag: missing 'tag' parameter")?;
                let descriptor = self
                    .edge_fields
                    .get("tags")
                    .ok_or("add_tag: 'tags' edge field is not registered on this provider")?;

                // Fail loud on a missing block with a clean message (the raw FK
                // violation from INSERT names no constraint).
                if !self.block_row_exists(id).await? {
                    return Err(format!("add_tag: block '{id}' not found").into());
                }

                // No-pages-under-non-pages guard (interim ruling 2026-07-13):
                // marking a block Page makes it a page, so its immediate parent
                // must be a page too (or `no_parent` — seed pages stay legal).
                if tag == PAGE_TAG {
                    let parent = self.read_real_parent_id(id).await?;
                    let parent_is_page = match &parent {
                        Some(pid) => Some(self.block_is_page(pid).await?),
                        None => None,
                    };
                    if holon_core::block_op_catalog::page_under_non_page_prohibited(
                        true,
                        parent_is_page,
                    ) {
                        return Err(holon_core::block_op_catalog::add_page_tag_rejection(
                            id,
                            parent.as_deref().unwrap_or(""),
                        )
                        .into());
                    }
                }

                // Atomic element-wise insert against the (block_id, tag) PK.
                // `affected == 1` ⇒ the tag was ABSENT (a real change);
                // `affected == 0` ⇒ already present (idempotent no-op). Because
                // this is a single atomic statement (not a read-then-write),
                // two concurrent same-tag adds cannot both observe "absent" and
                // double-journal — closing that window.
                let stmt = format!(
                    "INSERT OR IGNORE INTO {jt} ({sc}, {tc}) VALUES ('{b}', '{t}')",
                    jt = descriptor.join_table,
                    sc = Self::quote_identifier(&descriptor.source_col),
                    tc = Self::quote_identifier(&descriptor.target_col),
                    b = id.replace('\'', "''"),
                    t = tag.replace('\'', "''"),
                );
                let affected = self
                    .db_handle
                    .execute(&stmt, vec![])
                    .await
                    .map_err(|e| format!("add_tag: {e}"))?;

                let inverse = self.tag_inverse("remove_tag", id, tag);
                // `tags` is a junction field the column-only staleness reader
                // cannot fingerprint, so deltas are `history_only`: recorded in
                // history, excluded from the undo precondition. A no-op re-add
                // reports a VACUOUS delta (old == new) so the engine journals no
                // undo entry — undoing an idempotent re-add must never strip a
                // tag that was already present.
                let changes = if affected >= 1 {
                    vec![FieldDelta::history_only(
                        id,
                        "tags",
                        Value::Null,
                        Value::String(tag.to_string()),
                    )]
                } else {
                    vec![FieldDelta::history_only(
                        id,
                        "tags",
                        Value::String(tag.to_string()),
                        Value::String(tag.to_string()),
                    )]
                };
                Ok(OperationResult::new(changes, inverse))
            }
            "remove_tag" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or("remove_tag: missing 'id' parameter")?;
                let tag = params
                    .get("tag")
                    .and_then(|v| v.as_string())
                    .ok_or("remove_tag: missing 'tag' parameter")?;
                let descriptor = self
                    .edge_fields
                    .get("tags")
                    .ok_or("remove_tag: 'tags' edge field is not registered on this provider")?;

                if !self.block_row_exists(id).await? {
                    return Err(format!("remove_tag: block '{id}' not found").into());
                }

                // Unmarking a Page whose direct children are pages would orphan
                // them as pages under a non-page block — reject loud (cascade
                // unmark is a surprising bulk mutation, deliberately not done).
                if tag == PAGE_TAG
                    && self.block_is_page(id).await?
                    && self.has_page_child(id).await?
                {
                    return Err(holon_core::block_op_catalog::remove_page_tag_rejection(id).into());
                }

                // Targeted atomic delete. `affected == 1` ⇒ tag was present
                // (a real change); `affected == 0` ⇒ absent (idempotent no-op).
                let stmt = format!(
                    "DELETE FROM {jt} WHERE {sc} = '{b}' AND {tc} = '{t}'",
                    jt = descriptor.join_table,
                    sc = Self::quote_identifier(&descriptor.source_col),
                    tc = Self::quote_identifier(&descriptor.target_col),
                    b = id.replace('\'', "''"),
                    t = tag.replace('\'', "''"),
                );
                let affected = self
                    .db_handle
                    .execute(&stmt, vec![])
                    .await
                    .map_err(|e| format!("remove_tag: {e}"))?;

                let inverse = self.tag_inverse("add_tag", id, tag);
                let changes = if affected >= 1 {
                    vec![FieldDelta::history_only(
                        id,
                        "tags",
                        Value::String(tag.to_string()),
                        Value::Null,
                    )]
                } else {
                    vec![FieldDelta::history_only(
                        id,
                        "tags",
                        Value::Null,
                        Value::Null,
                    )]
                };
                Ok(OperationResult::new(changes, inverse))
            }
            _ => Err(format!("Unknown operation: {}", op_name).into()),
        }
    }

    /// Execute a batch in a single transaction.
    ///
    /// The `origin` argument is part of the `OriginTaggedWrites` write API
    /// (callers such as `LoroSyncController` tag their outbound batches
    /// `EventOrigin::Loro`), but the SQL writer no longer consumes it:
    /// provenance for echo-suppression rides the `_change_origin` CDC column
    /// via the trace context, not the (now-removed) EventBus.
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

            // block_links junction (links increment 2): the single-op
            // create/update paths derive it from the `marks` param via
            // `block_link_statements`, but that call lives OUTSIDE `prepare_*`,
            // so the batch path — the Loro→SQL projection sink
            // (`execute_batch_with_origin`, the DEFAULT Loro/Upstream app
            // wiring) — never populated the junction. Result (dogfood row 32):
            // `block_raw.marks` written but `block_links` EMPTY, so wiki-links
            // render as literal text and backlinks are impossible. Derive it
            // here for the same block create/update ops the single-op paths do.
            if self.entity_name == "block"
                && matches!(op_name.as_str(), "create" | "update")
                && let Some(id) = params.get("id").and_then(|v| v.as_string())
            {
                if let Some(marks) = params.get("marks") {
                    edge_sql.extend(self.block_link_statements(id, marks).await?);
                }
                if Self::params_tag_page(params)
                    && let Some(content) = params.get("content").and_then(|v| v.as_string())
                {
                    edge_sql.extend(Self::page_reresolve_statements(id, content));
                }
                // Same reasoning for block_redirects: under Loro authority
                // `merged_from` reaches SQL as a flattened property on this
                // batch row, never through the single-op `set_field` seam.
                if let Some(merged_from) = params.get(merge_blocks_plan::MERGED_FROM_FIELD) {
                    edge_sql.extend(Self::block_redirect_statements(id, merged_from)?);
                }
            }
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
        tracing::debug!(
            "[SqlOperationProvider] Executing batch: {} operations, {} SQL statements",
            count,
            all_sql.len()
        );
        let _tx_t0 = std::time::Instant::now();
        let _sql_count = all_sql.len();
        self.db_handle.transaction(all_sql).await.map_err(|e| {
            // Enrich with the op/id manifest — a deferred-FK failure only
            // surfaces at COMMIT with no row context, which made the
            // 2026-07-11 keystone FK RED undiagnosable from the log alone.
            let manifest: Vec<String> = operations
                .iter()
                .take(40)
                .map(|(op, p)| {
                    let id = p.get("id").and_then(|v| v.as_string()).unwrap_or("?");
                    let parent = p
                        .get("parent_id")
                        .and_then(|v| v.as_string())
                        .unwrap_or("-");
                    format!("{op}:{id}<-{parent}")
                })
                .collect();
            format!(
                "Batch transaction failed: {} (ops[{}]: {})",
                e,
                count,
                manifest.join(", ")
            )
        })?;
        tracing::debug!(
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
    use std::sync::Arc;

    use holon_api::TestClock;

    use super::*;

    /// An injected clock drives the write-time `created_at`/`updated_at`
    /// timestamps instead of the ambient system clock.
    #[tokio::test]
    async fn with_clock_stamps_injected_timestamp() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        db_handle
            .execute(
                "CREATE TABLE block_raw (id TEXT PRIMARY KEY, created_at INTEGER, updated_at \
                 INTEGER)",
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
mod delete_inverse_classification_tests {
    use holon_core::traits::UndoAction;
    use holon_turso::schema_modules::LinkSchemaModule;

    use super::*;
    use crate::storage::SchemaModule;

    async fn provider_with_rows() -> (crate::storage::turso::DbHandle, SqlOperationProvider) {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        // The `backlinks` matview (`LinkSchemaModule`) projects the WHOLE
        // block_raw row, so bind the production DDL — a hand-listed subset is
        // accepted at CREATE and only fails when the matview is read.
        for stmt in holon_turso::sql_utils::sql_statements(
            holon_turso::schema_modules::block_raw_schema_sql(),
        ) {
            db_handle.execute_ddl(stmt).await.expect("block_raw schema");
        }
        // The `delete` cascade cleans up `block_links` (drop outbound, un-resolve
        // inbound), so the table must exist. Reuse the canonical DDL via the
        // schema module rather than a duplicated string literal.
        LinkSchemaModule
            .ensure_schema(&db_handle)
            .await
            .expect("block_links schema");
        // The production block_raw carries a parent FK, so the fixture needs a
        // real root chain: the sentinel anchor plus the `block:root` every test
        // below parents its seeds under.
        for sql in [
            "INSERT OR IGNORE INTO block_raw (id, parent_id) VALUES \
             ('sentinel:no_parent', 'sentinel:no_parent')",
            "INSERT OR IGNORE INTO block_raw (id, parent_id) VALUES ('block:root', \
             'sentinel:no_parent')",
        ] {
            db_handle
                .execute(sql, vec![])
                .await
                .expect("seed root chain");
        }
        let provider = SqlOperationProvider::new(
            db_handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
        );
        // Leak the backend for the test's lifetime so the actor stays alive.
        std::mem::forget(_backend);
        (db_handle, provider)
    }

    async fn insert(provider: &SqlOperationProvider, id: &str, parent_id: &str) {
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("parent_id".into(), Value::String(parent_id.to_string()));
        params.insert("content".into(), Value::String(format!("content-{id}")));
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "create",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect("seed create");
    }

    async fn delete_result(provider: &SqlOperationProvider, id: &str) -> OperationResult {
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String(id.to_string()));
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "delete",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect("delete")
    }

    /// A LEAF delete carries a real identity-preserving inverse: a `create` of
    /// the same id.
    #[tokio::test]
    async fn leaf_delete_inverse_is_create_of_same_id() {
        let (_db, provider) = provider_with_rows().await;
        insert(&provider, "block:leaf", "block:root").await;

        let result = delete_result(&provider, "block:leaf").await;
        match result.undo {
            UndoAction::Undo(op) => {
                assert_eq!(op.op_name, "create");
                assert_eq!(
                    op.params.get("id").and_then(|v| v.as_string()),
                    Some("block:leaf"),
                    "inverse create must restore the same id"
                );
            }
            other => panic!("leaf delete must be reversible, got {other:?}"),
        }
    }

    /// A delete that CASCADES to descendants is DECLARED irreversible (subtree
    /// capture is a Wave-2 concern) — never an `Undeclared` loud-error, never a
    /// silent no-inverse.
    #[tokio::test]
    async fn subtree_delete_is_declared_irreversible() {
        let (_db, provider) = provider_with_rows().await;
        insert(&provider, "block:parent", "block:root").await;
        insert(&provider, "block:child", "block:parent").await;

        let result = delete_result(&provider, "block:parent").await;
        match result.undo {
            UndoAction::DeclaredIrreversible(reason) => {
                assert!(
                    reason.contains("subtree"),
                    "reason should name the subtree limitation, got {reason:?}"
                );
            }
            other => panic!("subtree delete must be declared irreversible, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod link_resolution_rewrite_tests {
    use holon_core::traits::UndoAction;
    use holon_turso::schema_modules::LinkSchemaModule;

    use super::*;
    use crate::storage::SchemaModule;

    async fn provider_with_links() -> (crate::storage::turso::DbHandle, SqlOperationProvider) {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        // The `backlinks` matview (`LinkSchemaModule`) projects the WHOLE
        // block_raw row, so bind the production DDL — a hand-listed subset is
        // accepted at CREATE and only fails when the matview is read.
        for stmt in holon_turso::sql_utils::sql_statements(
            holon_turso::schema_modules::block_raw_schema_sql(),
        ) {
            db_handle.execute_ddl(stmt).await.expect("block_raw schema");
        }
        LinkSchemaModule
            .ensure_schema(&db_handle)
            .await
            .expect("block_links schema");
        let provider = SqlOperationProvider::new(
            db_handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
        );
        std::mem::forget(_backend);
        (db_handle, provider)
    }

    async fn seed_link(db: &crate::storage::turso::DbHandle, source: &str, resolved: Option<&str>) {
        let rid = match resolved {
            Some(r) => format!("'{r}'"),
            None => "NULL".to_string(),
        };
        db.execute(
            &format!(
                "INSERT INTO block_links (source_block_id, target, kind, resolved_id) VALUES \
                 ('{source}', 'SomePage', 'page', {rid})"
            ),
            vec![],
        )
        .await
        .expect("seed link");
    }

    async fn resolved_of(db: &crate::storage::turso::DbHandle, source: &str) -> Value {
        let rows = db
            .query(
                &format!("SELECT resolved_id FROM block_links WHERE source_block_id = '{source}'"),
                HashMap::new(),
            )
            .await
            .expect("query resolved");
        rows.into_iter()
            .next()
            .and_then(|mut r| r.remove("resolved_id"))
            .expect("one row")
    }

    async fn rewrite(provider: &SqlOperationProvider, from: &str, to: &str) -> OperationResult {
        let mut params = StorageEntity::new();
        params.insert("from".into(), Value::String(from.to_string()));
        params.insert("to".into(), Value::String(to.to_string()));
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "rewrite_link_resolution",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect("rewrite_link_resolution")
    }

    async fn replay(provider: &SqlOperationProvider, op: &Operation) {
        let params: StorageEntity = op
            .params
            .iter()
            .map(|(k, v)| (k.as_str().into(), v.clone()))
            .collect();
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                &op.op_name,
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect("replay inverse");
    }

    /// The forward rewrite re-points every row resolved to `origin` onto `P`,
    /// and its inverse restores each affected row's prior `resolved_id`
    /// exactly.
    #[tokio::test]
    async fn rewrite_and_undo_restores_prior_resolved_ids() {
        let (db, provider) = provider_with_links().await;
        seed_link(&db, "block:src1", Some("block:origin")).await;
        seed_link(&db, "block:src2", Some("block:origin")).await;
        seed_link(&db, "block:src3", Some("block:other")).await;
        seed_link(&db, "block:src4", None).await;

        let result = rewrite(&provider, "block:origin", "block:P").await;

        // Inbound origin-links now resolve to P; unrelated rows untouched.
        assert_eq!(
            resolved_of(&db, "block:src1").await,
            Value::String("block:P".into())
        );
        assert_eq!(
            resolved_of(&db, "block:src2").await,
            Value::String("block:P".into())
        );
        assert_eq!(
            resolved_of(&db, "block:src3").await,
            Value::String("block:other".into())
        );
        assert_eq!(resolved_of(&db, "block:src4").await, Value::Null);

        // Inverse shape: restore_link_resolution carrying the two captured rows.
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("rewrite must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "restore_link_resolution");
        match inverse.params.get("rows") {
            Some(Value::Array(rows)) => assert_eq!(rows.len(), 2, "two origin rows captured"),
            other => panic!("rows must be an Array, got {other:?}"),
        }

        replay(&provider, &inverse).await;

        // Every row is back to its pre-rewrite resolved_id.
        assert_eq!(
            resolved_of(&db, "block:src1").await,
            Value::String("block:origin".into())
        );
        assert_eq!(
            resolved_of(&db, "block:src2").await,
            Value::String("block:origin".into())
        );
        assert_eq!(
            resolved_of(&db, "block:src3").await,
            Value::String("block:other".into())
        );
        assert_eq!(resolved_of(&db, "block:src4").await, Value::Null);
    }

    /// The inverse must be capture-based, NOT a blind `to → from` swap: a row
    /// that ALREADY resolved to `P` before the rewrite must survive the undo
    /// unchanged (a swap would wrongly re-point it to `origin`).
    #[tokio::test]
    async fn undo_does_not_touch_rows_preexisting_at_target() {
        let (db, provider) = provider_with_links().await;
        seed_link(&db, "block:moved", Some("block:origin")).await;
        seed_link(&db, "block:already", Some("block:P")).await;

        let result = rewrite(&provider, "block:origin", "block:P").await;
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("expected reversible, got {other:?}"),
        };
        replay(&provider, &inverse).await;

        assert_eq!(
            resolved_of(&db, "block:moved").await,
            Value::String("block:origin".into()),
            "the rewritten row returns to origin"
        );
        assert_eq!(
            resolved_of(&db, "block:already").await,
            Value::String("block:P".into()),
            "the pre-existing P row must be left untouched by undo"
        );
    }
}

#[cfg(test)]
mod two_phase_fk_tests {
    use std::collections::HashMap;

    use super::*;

    /// Face-A regression: a create batch containing a `requires` pair — the
    /// DEPENDENT block ordered BEFORE its required target — must apply
    /// FK-clean. Before the rows-then-edges two-phase split, the
    /// dependent's junction INSERT ran before the target's `block_raw` row
    /// existed, FK-rejecting and rolling back the WHOLE transaction (losing
    /// BOTH blocks). The two-phase apply writes every `block_raw` row
    /// before any junction, so op-vec order is irrelevant.
    #[tokio::test]
    async fn requires_pair_batch_applies_fk_clean_regardless_of_op_order() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        for ddl in [
            "PRAGMA foreign_keys = ON",
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, content TEXT, created_at INTEGER, \
             updated_at INTEGER)",
            "CREATE TABLE block_requires (block_id TEXT NOT NULL, required_id TEXT NOT NULL, \
             PRIMARY KEY (block_id, required_id), FOREIGN KEY (block_id) REFERENCES block_raw(id) \
             ON DELETE CASCADE, FOREIGN KEY (required_id) REFERENCES block_raw(id) ON DELETE \
             CASCADE)",
        ] {
            db_handle.execute(ddl, vec![]).await.expect("ddl");
        }

        // Precondition: FK enforcement is actually live in this environment —
        // a junction row referencing absent blocks must be REJECTED. Without
        // this, the test below would pass vacuously.
        db_handle
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:ghostA', \
                 'block:ghostB')",
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

#[cfg(test)]
mod tag_op_tests {
    use holon_core::traits::UndoAction;

    use super::*;

    fn tags_edge() -> EdgeFieldDescriptor {
        EdgeFieldDescriptor {
            entity: "block".to_string(),
            field: "tags".to_string(),
            join_table: "block_tags".to_string(),
            source_col: "block_id".to_string(),
            target_col: "tag".to_string(),
        }
    }

    async fn provider() -> (crate::storage::turso::DbHandle, SqlOperationProvider) {
        let (_backend, db) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        db.execute(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, sort_key \
             TEXT, depth INTEGER, properties TEXT, created_at INTEGER, updated_at INTEGER)",
            vec![],
        )
        .await
        .expect("block_raw");
        db.execute(
            "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY \
             KEY(block_id, tag), FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE)",
            vec![],
        )
        .await
        .expect("block_tags");
        let provider = SqlOperationProvider::with_edge_fields(
            db.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
            vec![tags_edge()],
        );
        std::mem::forget(_backend);
        (db, provider)
    }

    async fn seed_block(db: &crate::storage::turso::DbHandle, id: &str, parent_id: &str) {
        db.execute(
            &format!(
                "INSERT INTO block_raw (id, parent_id, content) VALUES ('{id}', '{parent_id}', \
                 'c-{id}')"
            ),
            vec![],
        )
        .await
        .expect("seed block");
    }

    async fn run(
        provider: &SqlOperationProvider,
        op: &str,
        id: &str,
        tag: &str,
    ) -> OperationResult {
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("tag".into(), Value::String(tag.to_string()));
        provider
            .execute_operation_with_origin(
                &EntityName::new("block"),
                op,
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .unwrap_or_else(|e| panic!("{op} failed: {e}"))
    }

    async fn tags_of(db: &crate::storage::turso::DbHandle, id: &str) -> Vec<String> {
        let rows = db
            .query(
                &format!("SELECT tag FROM block_tags WHERE block_id = '{id}' ORDER BY tag"),
                HashMap::new(),
            )
            .await
            .expect("query tags");
        rows.iter()
            .map(|r| {
                r.get("tag")
                    .and_then(|v| v.as_string())
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn is_vacuous(r: &OperationResult) -> bool {
        !r.changes.is_empty() && r.changes.iter().all(|d| d.old_value == d.new_value)
    }

    /// A second add of a present tag is an idempotent no-op: the junction still
    /// has exactly one row, and the result reports a VACUOUS delta so the
    /// engine journals no undo entry. (This also verifies the fork's INSERT
    /// OR IGNORE affected-row count — a broken count would surface the
    /// second add as non-vacuous.)
    #[tokio::test]
    async fn add_tag_is_idempotent() {
        let (db, p) = provider().await;
        seed_block(&db, "block:x", "block:root").await;

        let first = run(&p, "add_tag", "block:x", "todo").await;
        assert_eq!(tags_of(&db, "block:x").await, vec!["todo".to_string()]);
        assert!(!is_vacuous(&first), "first add is a real change");
        assert_eq!(first.changes[0].old_value, Value::Null);
        assert_eq!(first.changes[0].new_value, Value::String("todo".into()));

        let second = run(&p, "add_tag", "block:x", "todo").await;
        assert_eq!(
            tags_of(&db, "block:x").await,
            vec!["todo".to_string()],
            "re-adding must not duplicate"
        );
        assert!(is_vacuous(&second), "re-add of a present tag is vacuous");
    }

    /// remove_tag deletes only the named tag; siblings survive. Removing an
    /// absent tag is a vacuous no-op.
    #[tokio::test]
    async fn remove_tag_is_targeted_and_idempotent() {
        let (db, p) = provider().await;
        seed_block(&db, "block:x", "block:root").await;
        run(&p, "add_tag", "block:x", "a").await;
        run(&p, "add_tag", "block:x", "b").await;

        let removed = run(&p, "remove_tag", "block:x", "a").await;
        assert_eq!(tags_of(&db, "block:x").await, vec!["b".to_string()]);
        assert!(!is_vacuous(&removed));
        assert_eq!(removed.changes[0].old_value, Value::String("a".into()));
        assert_eq!(removed.changes[0].new_value, Value::Null);

        let noop = run(&p, "remove_tag", "block:x", "a").await;
        assert!(is_vacuous(&noop), "removing an absent tag is vacuous");
        assert_eq!(tags_of(&db, "block:x").await, vec!["b".to_string()]);
    }

    /// add_tag's inverse is remove_tag of the same {id, tag}; executing it
    /// undoes the add. Round-trip leaves the junction empty.
    #[tokio::test]
    async fn add_tag_inverse_round_trips() {
        let (db, p) = provider().await;
        seed_block(&db, "block:x", "block:root").await;
        let result = run(&p, "add_tag", "block:x", "todo").await;

        let inverse = match result.undo {
            UndoAction::Undo(op) => op,
            other => panic!("add_tag must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "remove_tag");
        let mut inv_params = StorageEntity::new();
        for (k, v) in &inverse.params {
            inv_params.insert(k.as_str().into(), v.clone());
        }
        p.execute_operation_with_origin(
            &EntityName::new("block"),
            &inverse.op_name,
            inv_params,
            EventOrigin::Other("test".to_string()),
        )
        .await
        .expect("replay inverse");
        assert!(tags_of(&db, "block:x").await.is_empty());
    }

    #[tokio::test]
    async fn add_tag_on_missing_block_fails_loud() {
        let (_db, p) = provider().await;
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String("block:ghost".to_string()));
        params.insert("tag".into(), Value::String("todo".to_string()));
        let err = p
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "add_tag",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect_err("add_tag on a missing block must fail loud");
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    /// Page guard: marking a block Page under a non-page parent is rejected;
    /// under a Page parent it is allowed; a block at `no_parent` (seed page) is
    /// allowed.
    #[tokio::test]
    async fn add_page_tag_nesting_guard() {
        let (db, p) = provider().await;
        seed_block(&db, "block:parent", "sentinel:no_parent").await;
        seed_block(&db, "block:child", "block:parent").await;

        // Parent is non-page → reject.
        let err = {
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String("block:child".to_string()));
            params.insert("tag".into(), Value::String(PAGE_TAG.to_string()));
            p.execute_operation_with_origin(
                &EntityName::new("block"),
                "add_tag",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect_err("page under non-page must be rejected")
        };
        assert!(
            err.to_string().contains("pages under non-pages"),
            "got: {err}"
        );

        // Seed page at no_parent → allowed.
        run(&p, "add_tag", "block:parent", PAGE_TAG).await;
        assert!(
            tags_of(&db, "block:parent")
                .await
                .contains(&PAGE_TAG.to_string())
        );

        // Now the parent IS a page → tagging the child Page is allowed.
        run(&p, "add_tag", "block:child", PAGE_TAG).await;
        assert!(
            tags_of(&db, "block:child")
                .await
                .contains(&PAGE_TAG.to_string())
        );
    }

    /// remove_tag("Page") is rejected when a direct child is itself a page.
    #[tokio::test]
    async fn remove_page_tag_with_page_child_rejected() {
        let (db, p) = provider().await;
        seed_block(&db, "block:parent", "sentinel:no_parent").await;
        seed_block(&db, "block:child", "block:parent").await;
        run(&p, "add_tag", "block:parent", PAGE_TAG).await;
        run(&p, "add_tag", "block:child", PAGE_TAG).await;

        let err = {
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String("block:parent".to_string()));
            params.insert("tag".into(), Value::String(PAGE_TAG.to_string()));
            p.execute_operation_with_origin(
                &EntityName::new("block"),
                "remove_tag",
                params,
                EventOrigin::Other("test".to_string()),
            )
            .await
            .expect_err("removing Page with a page child must be rejected")
        };
        assert!(
            err.to_string().contains("page under a non-page"),
            "got: {err}"
        );
    }
}
