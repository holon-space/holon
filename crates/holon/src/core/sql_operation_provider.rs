//! SQL-based operation provider for blocks
//!
//! Provides direct SQL access to block operations, bypassing the Loro CRDT layer.
//! Used when OrgMode is enabled but Loro is disabled, or by any component that
//! needs to write blocks directly to the database.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::datasource::{OperationProvider, OperationResult, Result};
use crate::storage::schema_module::EdgeFieldDescriptor;
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::DbHandle;
use crate::storage::types::StorageEntity;
use crate::sync::event_bus::{AggregateType, Event, EventBus, EventKind, EventOrigin};
use holon_api::{EntityName, OperationDescriptor, OperationParam, TypeHint, Value};

fn value_to_json(v: &Value) -> serde_json::Value {
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

/// A prepared operation: SQL statements and events, ready for execution.
struct PreparedOp {
    sql_statements: Vec<String>,
    events: Vec<Event>,
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
    event_bus: Option<Arc<dyn EventBus>>,
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
            event_bus: None,
        }
    }

    pub fn with_event_bus(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
        event_bus: Arc<dyn EventBus>,
    ) -> Self {
        Self::with_event_bus_and_edge_fields(
            db_handle,
            table_name,
            entity_name,
            entity_short_name,
            event_bus,
            Vec::new(),
        )
    }

    /// Same as `with_event_bus` plus an edge-field registry.
    pub fn with_event_bus_and_edge_fields(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
        event_bus: Arc<dyn EventBus>,
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
            event_bus: Some(event_bus),
        }
    }

    fn aggregate_type(&self) -> AggregateType {
        AggregateType::parse(&self.entity_short_name)
    }

    async fn publish_event(
        &self,
        event_kind: EventKind,
        aggregate_id: &str,
        payload: HashMap<String, serde_json::Value>,
    ) {
        if let Some(ref bus) = self.event_bus {
            let event = Event::new(
                event_kind,
                self.aggregate_type(),
                aggregate_id,
                EventOrigin::Other("sql".to_string()),
                payload,
            );
            if let Err(e) = bus.publish(event, None).await {
                tracing::warn!(
                    "[SqlOperationProvider] Failed to publish {}: {}",
                    event_kind,
                    e
                );
            }
        }
    }

    /// Find the document ancestor for a block using a single recursive CTE.
    ///
    /// Returns the block ID of the nearest ancestor with `name IS NOT NULL`
    /// (document blocks have names, content blocks don't). This replaces the
    /// previous O(depth) parent chain walk that fired one SELECT per ancestor.
    async fn find_document_uri(&self, block_id: &str) -> Option<String> {
        // A page is a block that has a 'Page' tag in the block_tags junction table.
        // Walk up the parent chain until one matches.
        let sql = format!(
            "WITH RECURSIVE chain(id, parent_id, is_page, depth) AS ( \
                SELECT b.id, b.parent_id, \
                    CASE WHEN bt.block_id IS NOT NULL THEN 1 ELSE 0 END as is_page, \
                    0 \
                FROM {table} b \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE b.id = '{block_id}' \
                UNION ALL \
                SELECT b.id, b.parent_id, \
                    CASE WHEN bt.block_id IS NOT NULL THEN 1 ELSE 0 END as is_page, \
                    c.depth + 1 \
                FROM {table} b \
                JOIN chain c ON b.id = c.parent_id \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE c.is_page = 0 AND c.depth < 50 \
            ) \
            SELECT id FROM chain WHERE is_page = 1 LIMIT 1",
            table = self.table_name,
            block_id = block_id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .expect("database query in find_document_uri must succeed");
        rows.first()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
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
        let is_source = params
            .get("content_type")
            .and_then(|v| v.as_string())
            .map_or(false, |s| s == "source");

        for (key, value) in params.iter() {
            if key == "properties" {
                // Capture existing properties JSON to merge with extras later
                if let Some(s) = value.as_string() {
                    existing_properties_json = Some(s.to_string());
                }
            } else if key.starts_with('_') {
                // Routing metadata (e.g., _routing_doc_uri) — skip for SQL
            } else if let Some(descriptor) = self.edge_fields.get(key.as_str()) {
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
            } else if self.known_columns.contains(key.as_str()) {
                // Trim trailing whitespace from content — org files don't
                // preserve it, so storing untrimmed content would cause
                // permanent divergence between DB and org round-trips.
                let value = if key == "content" {
                    &Self::trimmed_content(value, is_source)
                } else {
                    value
                };
                sql_fields.push((key.clone(), Self::value_to_sql(value)));
            } else {
                extra_props.insert(key.clone(), value.clone());
            }
        }

        // Merge existing properties JSON into extra_props
        if let Some(json_str) = existing_properties_json {
            if let Ok(map) = serde_json::from_str::<
                std::collections::HashMap<String, serde_json::Value>,
            >(&json_str)
            {
                for (k, v) in map {
                    if !extra_props.contains_key(&k) {
                        let value = match v {
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
                        };
                        extra_props.insert(k, value);
                    }
                }
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

    /// Execute a prepared operation: run SQL statements and publish events.
    async fn execute_prepared(&self, prepared: PreparedOp) -> Result<()> {
        for sql in &prepared.sql_statements {
            self.db_handle
                .execute(sql, vec![])
                .await
                .map_err(|e| format!("Failed to execute SQL: {}", e))?;
        }
        for event in prepared.events {
            if let Some(ref bus) = self.event_bus {
                if let Err(e) = bus.publish(event, None).await {
                    tracing::warn!("[SqlOperationProvider] Failed to publish event: {}", e);
                }
            }
        }
        Ok(())
    }

    fn make_event(
        &self,
        kind: EventKind,
        aggregate_id: &str,
        payload: HashMap<String, serde_json::Value>,
    ) -> Event {
        Event::new(
            kind,
            self.aggregate_type(),
            aggregate_id,
            EventOrigin::Other("sql".to_string()),
            payload,
        )
    }

    /// Build SQL + event for a create operation without executing.
    fn prepare_create(&self, params: &StorageEntity) -> PreparedOp {
        // Ensure timestamps are present so the event payload is a complete Block.
        // Without this, CacheEventSubscriber fails to deserialize: "missing field created_at".
        let mut params = params.clone();
        let now_ms = crate::util::now_unix_millis();
        params
            .entry("created_at".to_string())
            .or_insert_with(|| Value::Integer(now_ms));
        params
            .entry("updated_at".to_string())
            .or_insert_with(|| Value::Integer(now_ms));

        let (mut sql_fields, extra_props, edge_field_params) = self.partition_params(&params);

        if !extra_props.is_empty() {
            // BTreeMap for canonical key ordering (matches prepare_update).
            let props_json = serde_json::to_string(
                &extra_props
                    .into_iter()
                    .map(|(k, v)| (k, value_to_json(&v)))
                    .collect::<std::collections::BTreeMap<String, serde_json::Value>>(),
            )
            .unwrap_or_else(|_| "{}".to_string());
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

        let mut sql_statements = vec![format!(
            "INSERT OR IGNORE INTO {} ({}) VALUES ({})",
            self.table_name,
            columns.join(", "),
            values.join(", ")
        )];

        let aggregate_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or_default();

        // Edge-field rows: clear and reinsert per descriptor (no-op when no
        // edge fields are declared on this entity).
        for (descriptor, targets) in &edge_field_params {
            sql_statements.extend(Self::edge_field_replace_sql(
                aggregate_id,
                descriptor,
                targets,
            ));
        }

        let payload = self.build_event_payload(&params);
        let event = self.make_event(EventKind::Created, &aggregate_id, payload);

        PreparedOp {
            sql_statements,
            events: vec![event],
        }
    }

    /// Build SQL + event for an update operation without executing.
    /// Returns None if there are no fields to update.
    ///
    /// Async because it needs to look up parent_id for the event payload (so the
    /// OrgSyncController can route to on_block_changed instead of re_render_all_tracked).
    async fn prepare_update(&self, params: &StorageEntity) -> Result<Option<PreparedOp>> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("SqlOperationProvider::prepare_update: missing 'id' parameter");

        // Optional compare-and-set content. When the caller (Loro outbound
        // reconcile) embeds `_expected_content`, the UPDATE becomes conditional
        // on SQL still matching that value. Stops the outbound reconcile from
        // regressing SQL when a concurrent direct write (UI dispatch) has
        // advanced the row since the Loro snapshot was taken. Trimmed to match
        // the write-side normalization in `trimmed_content`.
        let expected_content = params
            .get("_expected_content")
            .and_then(|v| v.as_string())
            .map(|s| s.trim_end().to_string());

        // Same guard for parent_id: a stale Loro outbound carrying a
        // pre-image parent_id won't stomp a fresh local move. parent_id
        // values are not normalized, so no trim.
        let expected_parent_id = params
            .get("_expected_parent_id")
            .and_then(|v| v.as_string())
            .map(String::from);

        // Same guard for marks: a stale Loro outbound carrying a pre-image
        // marks JSON won't regress a fresh local mark edit. Compared as
        // raw strings — the JSON wire format from `marks_to_json` is
        // canonical (serde_json default ordering for fields, no whitespace).
        let expected_marks = params
            .get("_expected_marks")
            .and_then(|v| v.as_string())
            .map(String::from);

        let (sql_fields, extra_props, edge_field_params) = self.partition_params(params);

        // TRACE: any non-standard custom property being written via update path
        const STANDARD_PROP_KEYS: &[&str] = &[
            "task_state",
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
                existing.insert(k.clone(), value_to_json(v));
            }

            // Canonicalize key order so the diff guard's string comparison
            // matches regardless of insertion order across code paths.
            let canonical: std::collections::BTreeMap<_, _> = existing.into_iter().collect();
            let merged_json =
                serde_json::to_string(&canonical).expect("merged properties must serialize");
            let props_sql = format!("'{}'", merged_json.replace('\'', "''"));
            update_pairs.push(("properties".to_string(), props_sql));
        }

        if update_pairs.is_empty() && edge_field_params.is_empty() {
            return Ok(None);
        }

        let set_clauses: Vec<String> = update_pairs
            .iter()
            .map(|(k, v)| format!("{} = {}", Self::quote_identifier(k), v))
            .collect();

        let mut where_parts = vec![format!("id = '{}'", id.replace('\'', "''"))];
        if let Some(expected) = expected_content {
            where_parts.push(format!("content = '{}'", expected.replace('\'', "''")));
        }
        if let Some(expected) = expected_parent_id {
            where_parts.push(format!("parent_id = '{}'", expected.replace('\'', "''")));
        }
        if let Some(expected) = expected_marks {
            // Empty sentinel = pre-image was Block.marks=None → SQL row has
            // marks IS NULL. Otherwise compare canonical JSON exactly.
            if expected.is_empty() {
                where_parts.push("marks IS NULL".to_string());
            } else {
                where_parts.push(format!("marks = '{}'", expected.replace('\'', "''")));
            }
        }

        // Diff guard: prevent Turso IVM from firing CDC on no-op UPDATEs.
        // When the Loro outbound reconcile echoes back values already in SQL,
        // the UPDATE would touch 0 data columns but still trigger IVM CDC.
        // Adding `AND (col1 IS NOT val1 OR col2 IS NOT val2 ...)` makes the
        // UPDATE affect 0 rows when nothing changed, avoiding spurious CDC.
        // Exclude timestamp columns: `updated_at` is always set to `now` by
        // `block_to_params`, so it always differs — but a timestamp bump
        // alone is not a meaningful change worth CDC-notifying about.
        const DIFF_GUARD_SKIP: &[&str] = &["updated_at", "created_at"];
        let diff_conditions: Vec<String> = update_pairs
            .iter()
            .filter(|(k, _)| !DIFF_GUARD_SKIP.contains(&k.as_str()))
            .map(|(k, v)| format!("{} IS NOT {}", Self::quote_identifier(k), v))
            .collect();
        if !diff_conditions.is_empty() {
            where_parts.push(format!("({})", diff_conditions.join(" OR ")));
        }

        let mut sql_statements = Vec::new();
        if !update_pairs.is_empty() {
            sql_statements.push(format!(
                "UPDATE {} SET {} WHERE {}",
                self.table_name,
                set_clauses.join(", "),
                where_parts.join(" AND ")
            ));
        }
        for (descriptor, targets) in &edge_field_params {
            sql_statements.extend(Self::edge_field_replace_sql(&id, descriptor, targets));
        }
        Ok(Some(PreparedOp {
            sql_statements,
            events: vec![],
        }))
    }

    /// Build SQL + events for a delete operation (with cascade) without executing.
    /// Requires async because cascade discovery queries the DB.
    async fn prepare_delete(&self, params: &StorageEntity) -> Result<PreparedOp> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        let doc_uri = self.find_document_uri(&id).await;

        let mut queue = vec![id.to_string()];
        let mut all_ids = Vec::new();
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
            queue.extend(children.iter().cloned());
            all_ids.extend(children);
        }

        let delete_payload = match &doc_uri {
            Some(uri) => {
                let mut p = HashMap::new();
                p.insert(
                    crate::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                    serde_json::Value::String(uri.clone()),
                );
                p
            }
            None => HashMap::new(),
        };

        let mut sql_statements = Vec::new();
        let mut events = Vec::new();

        // Delete descendants bottom-up
        for desc_id in all_ids.iter().rev() {
            sql_statements.push(format!(
                "DELETE FROM {} WHERE id = '{}'",
                self.table_name,
                desc_id.replace('\'', "''")
            ));
            events.push(self.make_event(EventKind::Deleted, desc_id, delete_payload.clone()));
        }

        // Delete the target block itself
        sql_statements.push(format!(
            "DELETE FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''")
        ));
        events.push(self.make_event(EventKind::Deleted, &id, delete_payload));

        Ok(PreparedOp {
            sql_statements,
            events,
        })
    }

    /// Build an event payload from params that CacheEventSubscriber can deserialize as Block.
    ///
    /// The problem: `params` is flat — extra properties like `collapse-to` and `column-order`
    /// are top-level keys. But `Block` expects them nested under a `properties` key.
    /// Without this restructuring, `serde_json::from_value::<Block>` drops unknown keys
    /// and `properties` gets `{}` via `#[serde(default)]`, causing INSERT OR REPLACE
    /// in QueryableCache to overwrite the correct SQL data with empty properties.
    fn build_event_payload(&self, params: &StorageEntity) -> HashMap<String, serde_json::Value> {
        let mut payload = HashMap::new();

        let mut data_map = serde_json::Map::new();
        let mut props_map = serde_json::Map::new();

        let is_source = params
            .get("content_type")
            .and_then(|v| v.as_string())
            .map_or(false, |s| s == "source");

        for (key, value) in params.iter() {
            // Edge-typed fields live in junction tables, not in the row's
            // event payload. Skip so a Value::Array doesn't fall into the
            // debug-formatted fallback below.
            if self.edge_fields.contains_key(key.as_str()) {
                continue;
            }
            let value = if key == "content" {
                &Self::trimmed_content(value, is_source)
            } else {
                value
            };
            let json_val = value_to_json(value);

            // Skip NULL columns: serde's `#[serde(default)]` on Block fields
            // (tags, properties, marks, …) only fires for ABSENT keys, not
            // present-but-null. Routing metadata still propagates as null.
            let is_null = matches!(json_val, serde_json::Value::Null);

            // Keys starting with `_` are routing metadata — placed at the
            // top level of the event payload, not in the `data` object.
            if key.starts_with('_') {
                payload.insert(key.clone(), json_val);
            } else if is_null {
                continue;
            } else if key == "properties" {
                // Existing properties — merge into props_map.
                // Handles both String (raw JSON from SQL) and Object (parsed by Turso).
                match value {
                    Value::String(s) => {
                        if let Ok(map) =
                            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(s)
                        {
                            for (k, v) in map {
                                props_map.insert(k, v);
                            }
                        }
                    }
                    Value::Object(obj) => {
                        for (k, v) in obj {
                            props_map.insert(k.clone(), value_to_json(v));
                        }
                    }
                    _ => {}
                }
            } else if self.known_columns.contains(key.as_str()) {
                data_map.insert(key.clone(), json_val);
            } else {
                // Extra property — nest under `properties`
                props_map.insert(key.clone(), json_val);
            }
        }

        if !props_map.is_empty() {
            data_map.insert(
                "properties".to_string(),
                serde_json::Value::Object(props_map),
            );
        }

        if let Some(id_val) = params.get("id").and_then(|v| v.as_string()) {
            tracing::trace!(
                "[BUILD_EVENT_TRACE] id={} data_keys={:?} properties={:?}",
                id_val,
                data_map.keys().collect::<Vec<_>>(),
                data_map.get("properties")
            );
        }

        payload.insert("data".to_string(), serde_json::Value::Object(data_map));
        payload
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
                        .map_or(false, |s| s == "source");
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
                    for stmt in Self::edge_field_replace_sql(&id, descriptor, &targets) {
                        self.db_handle
                            .execute(&stmt, vec![])
                            .await
                            .map_err(|e| format!("Failed to execute edge-field SQL: {}", e))?;
                    }
                    return Ok(OperationResult::irreversible(Vec::new()));
                }

                let sql = if self.known_columns.contains(&*field) {
                    format!(
                        "UPDATE {} SET {} = {} WHERE id = '{}'",
                        self.table_name,
                        Self::quote_identifier(&field),
                        sql_value,
                        id.replace('\'', "''")
                    )
                } else if matches!(value, Value::Null) {
                    // Null means "remove this property" — use json_remove so we don't
                    // leave a {"key": null} entry in the JSON column.
                    format!(
                        "UPDATE {} SET properties = json_remove(COALESCE(properties, '{{}}'), '$.{}') WHERE id = '{}'",
                        self.table_name,
                        field.replace('\'', "''"),
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
                self.db_handle
                    .execute(&sql, vec![])
                    .await
                    .map_err(|e| format!("Failed to execute SQL: {}", e))?;

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

                let mut payload = HashMap::new();
                let fields_json =
                    serde_json::to_value(vec![(&field, &Value::Null, value)]).unwrap_or_default();
                payload.insert("fields".to_string(), fields_json);
                // Include _routing_doc_uri so the OrgMode event handler can route this
                // to on_block_changed(doc_id) instead of re_render_all_tracked().
                if let Some(doc_uri) = self.find_document_uri(&id).await {
                    payload.insert(
                        crate::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                        serde_json::Value::String(doc_uri),
                    );
                }
                self.publish_event(EventKind::FieldsChanged, &id, payload)
                    .await;

                Ok(OperationResult::irreversible(Vec::new()))
            }
            "create" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("create: missing 'id'")
                    .to_string();
                // Ensure timestamps are present so the Created event payload
                // deserializes as a complete Block in CacheEventSubscriber.
                // `prepare_create` injects them into its local clone, but the
                // outer `params` used for the event payload below also needs
                // them — otherwise the event drops and the cache stays stale.
                let mut params = params;
                let now_ms = crate::util::now_unix_millis();
                params
                    .entry("created_at".to_string())
                    .or_insert_with(|| Value::Integer(now_ms));
                params
                    .entry("updated_at".to_string())
                    .or_insert_with(|| Value::Integer(now_ms));
                let prepared = self.prepare_create(&params);
                // Execute SQL without publishing events — we'll publish
                // manually below with _routing_doc_uri attached.
                for sql in &prepared.sql_statements {
                    self.db_handle
                        .execute(sql, vec![])
                        .await
                        .map_err(|e| format!("Failed to execute SQL: {}", e))?;
                }
                // Publish Create event with routing info (block now exists in DB).
                let mut payload = self.build_event_payload(&params);
                if let Some(doc_uri) = self.find_document_uri(&id).await {
                    // Always override: build_event_payload may have copied a stale
                    // _routing_doc_uri from the caller's params (e.g. OrgSyncController
                    // passes the block's own ID, not the document ancestor).
                    payload.insert(
                        crate::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                        serde_json::Value::String(doc_uri),
                    );
                }
                self.publish_event(EventKind::Created, &id, payload).await;

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
                                .ok() // ALLOW(ok): id-collision fallback
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
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("update: missing 'id'")
                    .to_string();
                if let Some(prepared) = self.prepare_update(&params).await? {
                    self.execute_prepared(prepared).await?;
                }
                // Read the full row AFTER the UPDATE so the event carries complete data.
                let select_sql = format!(
                    "SELECT * FROM {} WHERE id = '{}'",
                    self.table_name,
                    id.replace('\'', "''")
                );
                let full_rows = self
                    .db_handle
                    .query(&select_sql, HashMap::new())
                    .await
                    .map_err(|e| format!("post-update row read for {}: {}", id, e))?;
                let full_row = full_rows.into_iter().next();
                let event_data = full_row.as_ref().unwrap_or(&params);
                let mut payload = self.build_event_payload(event_data);
                if let Some(doc_uri) = self.find_document_uri(&id).await {
                    payload.insert(
                        crate::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                        serde_json::Value::String(doc_uri),
                    );
                }
                self.publish_event(EventKind::Updated, &id, payload).await;
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
                let next = holon_api::render_eval::cycle_state(&current, &states);

                let mut set_params = StorageEntity::new();
                set_params.insert("id".into(), Value::String(id));
                set_params.insert("field".into(), Value::String("task_state".into()));
                set_params.insert("value".into(), Value::String(next));
                self.execute_operation(entity_name, "set_field", set_params)
                    .await
            }
            _ => Err(format!("Unknown operation: {}", op_name).into()),
        }
    }

    /// Execute multiple operations in a single transaction with batch event publishing.
    ///
    /// All SQL statements are wrapped in one transaction (single CDC/IVM pass).
    /// All events are published in a single batch after the transaction commits.
    async fn execute_batch(
        &self,
        entity_name: &EntityName,
        operations: Vec<(String, StorageEntity)>,
    ) -> Result<Vec<OperationResult>> {
        self.execute_batch_with_origin(
            entity_name,
            operations,
            EventOrigin::Other("sql".to_string()),
        )
        .await
    }

    /// Execute a batch and tag the resulting events with the given origin.
    ///
    /// The `prepare_*` helpers build events with a default origin; this method
    /// overrides that origin on all collected events before publishing. Used
    /// by `LoroSyncController` to tag its outbound batches as
    /// `EventOrigin::Loro` so the inbound direction can skip echoes.
    async fn execute_batch_with_origin(
        &self,
        entity_name: &EntityName,
        operations: Vec<(String, StorageEntity)>,
        origin: EventOrigin,
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

        // Phase 1: Prepare all operations (may involve async DB reads for delete cascade)
        let mut all_sql = Vec::new();
        let mut all_events = Vec::new();
        let mut update_ids = Vec::new();

        for (op_name, params) in &operations {
            let prepared = match op_name.as_str() {
                "create" => self.prepare_create(params),
                "update" => {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string();
                    update_ids.push(id);
                    match self.prepare_update(params).await? {
                        Some(p) => p,
                        None => continue,
                    }
                }
                "delete" => self.prepare_delete(params).await?,
                other => return Err(format!("Unknown batch operation: {}", other).into()),
            };
            all_sql.extend(prepared.sql_statements.into_iter().map(|s| (s, vec![])));
            all_events.extend(prepared.events);
        }

        let count = operations.len();

        // Phase 2: Execute all SQL in a single transaction
        tracing::info!(
            "[SqlOperationProvider] Executing batch: {} operations, {} SQL statements",
            count,
            all_sql.len()
        );
        self.db_handle
            .transaction(all_sql)
            .await
            .map_err(|e| format!("Batch transaction failed: {}", e))?;

        // Phase 2b: Build events for update ops by reading the post-update rows.
        // prepare_update doesn't emit events (params may be partial); we read
        // the full row after SQL execution for a complete event payload.
        for id in &update_ids {
            let select_sql = format!(
                "SELECT * FROM {} WHERE id = '{}'",
                self.table_name,
                id.replace('\'', "''")
            );
            let rows = self
                .db_handle
                .query(&select_sql, HashMap::new())
                .await
                .map_err(|e| format!("post-update row read for {}: {}", id, e))?;
            if let Some(row) = rows.into_iter().next() {
                let mut payload = self.build_event_payload(&row);
                if let Some(doc_uri) = self.find_document_uri(id).await {
                    payload.insert(
                        crate::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                        serde_json::Value::String(doc_uri),
                    );
                }
                all_events.push(self.make_event(EventKind::Updated, id, payload));
            }
        }

        // Override the default "sql" origin on all events before publish.
        for event in &mut all_events {
            event.origin = origin.clone();
        }

        // Phase 3: Publish all events in a single batch
        if let Some(ref bus) = self.event_bus {
            bus.publish_batch(all_events)
                .await
                .map_err(|e| format!("Batch event publish failed: {}", e))?;
        }

        Ok(vec![OperationResult::irreversible(Vec::new()); count])
    }
}

#[cfg(test)]
#[path = "sql_operation_provider_outbound_parent_test.rs"]
mod sql_operation_provider_outbound_parent_test;
