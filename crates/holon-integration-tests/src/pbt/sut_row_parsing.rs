//! Row-shape ↔ `Block` conversion + mutation-property projection helpers.
//!
//! @pbt kind cap-plumbing
//! @pbt covers sql-slice/frontend-slice — shared fail-loud SQL-row→Block parser
//!   so the SQL invariant path and LiveData mirror stay byte-for-byte
//! identical.
//!
//! - [`parse_block_row`] turns a SQL row from the `block` matview into a typed
//!   [`Block`]. Used by `inv-backend-blocks-match-ref`'s SQL path and the
//!   `LiveData<Block>` mirror.
//! - [`mutation_expected_properties`] + [`row_properties_to_map`] are used by
//!   the per-mutation property spot-check (inv-properties-match-mutation).
//!
//! Extracted from `sut.rs` (Phase D2).

use std::collections::HashMap;

use holon_api::ContentType;
use holon_api::SourceLanguage;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::OrgBlockExt;

/// Snapshot SQL for the `block` MATVIEW — the canonical projection that carries
/// the junction edge fields as `json_group_array` columns — one per
/// [`holon_api::EdgeField`].
/// Backs the `inv-blocks-match-ref/matview` reader
/// (`SutBackend::live_block_snapshot`). Centralised here, next to
/// [`parse_block_row`], so the column list and its parser stay in lockstep and
/// the SQL isn't duplicated across SUT impls.
pub(super) const BLOCK_MATVIEW_SNAPSHOT_SQL: &str = "SELECT id, parent_id, content, content_type, \
                                                     source_language, source_name, \
                                                     properties, marks, collapsed, \
                                                     widget_only, tags, requires, \
                                                     advice_suppressed, contributes_to FROM \
                                                     block";

/// Snapshot SQL for the write-side `block_raw` BASE TABLE. Native columns only
/// — `block_raw` has no junction `tags`/`requires`, so [`parse_block_row`]
/// leaves those empty and the `/block_raw` invariant compares a field subset.
/// Backs `SutBackend::block_raw_snapshot`.
///
/// Excludes the self-parented `sentinel:no_parent` FK-anchor row that
/// `CoreSchemaModule` seeds to satisfy the `block_raw.parent_id` FK — it is not
/// a real block and never appears in the reference model. Production's `block`
/// matview drops it the same way (`schema_modules.rs`: `WHERE b.id !=
/// 'sentinel:no_parent'`).
pub(super) const BLOCK_RAW_SNAPSHOT_SQL: &str = "SELECT id, parent_id, content, content_type, \
                                                 source_language, source_name, properties, \
                                                 marks, collapsed, widget_only FROM block_raw \
                                                 WHERE id != 'sentinel:no_parent'";

/// Read a `NOT NULL DEFAULT 0` SQLite boolean column that the snapshot SQL is
/// REQUIRED to select. Absent = the SQL lost the column, which would silently
/// pin the field to `false` for every block and make the invariant vacuous on
/// it — so that is a panic naming the constant to fix, not a default.
fn required_sql_bool(row: &holon_core::storage::types::StorageEntity, col: &str) -> bool {
    match row.get(col) {
        Some(Value::Integer(i)) => *i != 0,
        Some(Value::Boolean(b)) => *b,
        Some(other) => panic!("block row {col:?} must be an INTEGER 0/1, got {other:?}"),
        None => panic!(
            "block row is missing the {col:?} column — add it to BLOCK_MATVIEW_SNAPSHOT_SQL / \
             BLOCK_RAW_SNAPSHOT_SQL; without it every parsed Block reports {col}=false and \
             inv-blocks-match-ref is vacuous on that field"
        ),
    }
}

/// Read a NULLABLE TEXT column that the snapshot SQL is REQUIRED to select.
///
/// The three cases are genuinely distinct and must stay so: an ABSENT column
/// (`None`) means the SELECT lost it — a harness bug that would silently pin
/// the field for every block, so it panics naming the constants to fix; SQL
/// NULL (`Some(Value::Null)`) is a legitimate VALUE meaning "no source name"
/// and maps to `None`; text maps to `Some`. `StorageEntity::get` distinguishes
/// absent from NULL, which is the same distinction `required_sql_bool` above
/// and `Block::try_from`'s `optional_bool` both rely on.
fn required_sql_opt_string(
    row: &holon_core::storage::types::StorageEntity,
    col: &str,
) -> Option<String> {
    match row.get(col) {
        Some(Value::Null) => None,
        Some(v) => Some(
            v.as_string()
                .unwrap_or_else(|| panic!("block row {col:?} must be TEXT or NULL, got {v:?}"))
                .to_string(),
        ),
        None => panic!(
            "block row is missing the {col:?} column — add it to BLOCK_MATVIEW_SNAPSHOT_SQL / \
             BLOCK_RAW_SNAPSHOT_SQL; without it every parsed Block reports {col}=None and \
             inv-blocks-match-ref is vacuous on that field"
        ),
    }
}

/// Parse a batch of snapshot rows into typed [`Block`]s, fail-loud on any row
/// that won't parse (a malformed row is a bug, never silently skipped).
pub(super) fn parse_block_rows(rows: &[holon_core::storage::types::StorageEntity]) -> Vec<Block> {
    rows.iter()
        .map(|r| {
            parse_block_row(r)
                .unwrap_or_else(|| panic!("parse_block_row returned None for row {r:?}"))
        })
        .collect()
}

/// Build a `Block` from a SQL row that includes id/content/content_type/
/// source_language/parent_id/properties (and optionally tags + org fields).
/// Used both by inv-backend-blocks-match-ref's SQL path and by the
/// LiveData<Block> experiment so the two stay byte-for-byte equivalent.
pub(super) fn parse_block_row(row: &holon_core::storage::types::StorageEntity) -> Option<Block> {
    let id =
        EntityUri::parse(row.get("id")?.as_string()?).expect("block id from DB must be valid URI");
    // A NULL/missing `parent_id` in `block_raw` is a top-level block — semantically
    // `no_parent()` (the sentinel `no_orphan`/`block_parent` already recognize as
    // the valid root). Splitting a top-level block mints such a sibling;
    // coalescing here (rather than returning `None`, which every caller treats
    // as a hard error) lets the SQL slice parse top-level blocks instead of
    // panicking.
    let parent_id = match row.get("parent_id").and_then(|v| v.as_string()) {
        Some(s) => EntityUri::parse(s).expect("block parent_id from DB must be valid URI"),
        None => EntityUri::no_parent(),
    };
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let mut block = Block::new_text(id, parent_id, content);

    block.tags = row
        .get("tags")
        .map(|v| match v {
            Value::Array(arr) => arr
                .iter()
                .filter_map(|x| x.as_string().map(|s| s.to_string()))
                .collect(),
            Value::Json(s) | Value::String(s) => {
                serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
            }
            _ => Vec::new(),
        })
        .unwrap_or_default()
        .into();

    // The block-referencing edge fields are hydrated from their junctions by the
    // matview's json_group_array, exactly like `tags`. Omitting one silently
    // pins the SUT side to `[]`, so `inv-blocks-match-ref/matview` can never
    // see that edge fail to project.
    block.requires = edge_targets(row, "requires");
    block.advice_suppressed = edge_targets(row, "advice_suppressed");
    block.contributes_to = edge_targets(row, "contributes_to");

    // `marks` — inline rich-text marks JSON (dogfood 2026-07-10 link-destruction
    // class): parse it into `block.marks` or the SUT-side observable is
    // marks-blind and `inv-blocks-match-ref` can never see mark loss. Mirrors
    // `Block::try_from`'s arm: absent/Null/empty → None; invalid JSON = bug.
    block.marks =
        match row.get("marks") {
            None | Some(Value::Null) => None,
            Some(Value::Json(s)) | Some(Value::String(s)) => {
                if s.is_empty() {
                    None
                } else {
                    Some(holon_api::marks_from_json(s).unwrap_or_else(|e| {
                        panic!("block row 'marks' holds invalid JSON {s:?}: {e}")
                    }))
                }
            }
            Some(other) => panic!("block row 'marks' must be a JSON string, got {other:?}"),
        };

    // The typed FOLD scalars, for the same reason the edge fields above are
    // hydrated: leaving one out does not merely lose it, it pins the SUT side to
    // `false` for EVERY block, so `inv-blocks-match-ref` reports
    // `collapsed: sut=false ref=true` on any folded block no matter what the
    // store holds — and can never see a real fold regression either. Absent is
    // a LOUD error rather than a default: the only way a column goes missing is
    // that someone edited the snapshot SQL above, which is exactly the mistake
    // this arm exists to stop from recurring silently.
    block.collapsed = required_sql_bool(row, "collapsed");
    block.widget_only = required_sql_bool(row, "widget_only");

    if let Some(content_type) = row.get("content_type").and_then(|v| v.as_string()) {
        block.content_type = content_type.parse::<ContentType>().unwrap();
    }
    if let Some(source_language) = row.get("source_language").and_then(|v| v.as_string()) {
        block.source_language = Some(source_language.parse::<SourceLanguage>().unwrap());
    }
    // `source_name` is compared by `compare_block_fields` (`delta!(source_name)`)
    // and both tables store it, so leaving it unread pins the SUT side to `None`
    // and makes the comparison vacuous on it. Guarded like the fold scalars: a
    // NULL here is a real value (no source name), but an ABSENT column is the
    // SELECT having lost it, which is the failure this guard exists to catch.
    block.source_name = required_sql_opt_string(row, "source_name");

    if let Some(props_val) = row.get("properties") {
        match props_val {
            Value::String(s) => {
                if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(s) {
                    block.properties = map;
                }
            }
            Value::Object(props) => {
                for (k, v) in props {
                    block.properties.insert(k.clone(), v.clone());
                }
            }
            _ => {}
        }
    }

    if let Some(task_state) = row
        .get("task_state")
        .or_else(|| row.get("TODO"))
        .and_then(|v| v.as_string())
    {
        block.set_task_state(Some(holon_api::TaskState::from_keyword(task_state)));
    }
    if let Some(priority) = row
        .get("priority")
        .or_else(|| row.get("PRIORITY"))
        .and_then(|v| v.as_i64())
    {
        block.set_priority(Some(
            holon_api::Priority::from_int(priority as i32)
                .unwrap_or_else(|e| panic!("stored priority {priority} is invalid: {e}")),
        ));
    }
    // `block.tags` is already populated from the matview JSON column above
    // (the dual-LEFT json_group_array projection). The legacy CSV handler
    // is only relevant for the org-property column `TAGS` (which the
    // matview projects through `properties`, not as a top-level column).
    if let Some(tags) = row.get("TAGS").and_then(|v| v.as_string()) {
        block.set_tags(holon_api::Tags::from_csv(tags));
    }
    if let Some(scheduled) = row
        .get("scheduled")
        .or_else(|| row.get("SCHEDULED"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(scheduled)
    {
        block.set_scheduled(Some(ts));
    }
    if let Some(deadline) = row
        .get("deadline")
        .or_else(|| row.get("DEADLINE"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(deadline)
    {
        block.set_deadline(Some(ts));
    }

    Some(block)
}

/// One hydrated edge-field column as typed target URIs. The matview delivers a
/// `json_group_array`, which reaches here either already decoded
/// (`Value::Array`) or as its JSON text, depending on the driver path.
fn edge_targets(row: &holon_core::storage::types::StorageEntity, column: &str) -> Vec<EntityUri> {
    let Some(v) = row.get(column) else {
        return Vec::new();
    };
    let raw: Vec<String> = match v {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_string().map(|s| s.to_string()))
            .collect(),
        Value::Json(s) | Value::String(s) => serde_json::from_str::<Vec<String>>(s)
            .unwrap_or_else(|e| panic!("block row {column:?} holds invalid JSON {s:?}: {e}")),
        other => panic!("block row {column:?} must be a JSON array, got {other:?}"),
    };
    raw.into_iter()
        .map(|s| {
            EntityUri::parse_owned(s)
                .unwrap_or_else(|e| panic!("stored {column} entry is not a valid URI: {e}"))
        })
        .collect()
}
