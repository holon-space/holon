//! Row-shape ↔ `Block` conversion + mutation-property projection helpers.
//!
//! - [`parse_block_row`] turns a SQL row from the `block` matview into a
//!   typed [`Block`]. Used by `inv-backend-blocks-match-ref`'s SQL path
//!   and the `LiveData<Block>` mirror.
//! - [`mutation_expected_properties`] + [`row_properties_to_map`] are
//!   used by the per-mutation property spot-check (inv-properties-match-mutation).
//!
//! Extracted from `sut.rs` (Phase D2).

use std::collections::HashMap;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, SourceLanguage, Value};
use holon_orgmode::OrgBlockExt;

use super::types::Mutation;

/// Build a `Block` from a SQL row that includes id/content/content_type/
/// source_language/parent_id/properties (and optionally tags + org fields).
/// Used both by inv-backend-blocks-match-ref's SQL path and by the LiveData<Block> experiment so
/// the two stay byte-for-byte equivalent.
pub(super) fn parse_block_row(row: &holon::storage::types::StorageEntity) -> Option<Block> {
    let id =
        EntityUri::parse(row.get("id")?.as_string()?).expect("block id from DB must be valid URI");
    let parent_id = EntityUri::parse(row.get("parent_id")?.as_string()?)
        .expect("block parent_id from DB must be valid URI");
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
        .unwrap_or_default();

    if let Some(content_type) = row.get("content_type").and_then(|v| v.as_string()) {
        block.content_type = content_type.parse::<ContentType>().unwrap();
    }
    if let Some(source_language) = row.get("source_language").and_then(|v| v.as_string()) {
        block.source_language = Some(source_language.parse::<SourceLanguage>().unwrap());
    }

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

/// Fields that are SQL columns on `block` rather than entries in the
/// `properties` JSON column. When an External mutation's `fields` map contains
/// one of these, the expected effect lands in a column — not in `properties` —
/// so it's excluded from the post-mutation property spot-check.
const BLOCK_SQL_COLUMNS: &[&str] = &[
    "id",
    "parent_id",
    "name",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "collapsed",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
];

/// Extract the subset of a mutation's `fields` that should land in the DB
/// row's `properties` JSON column (i.e. custom properties and org drawer
/// props like `task_state`, `effort`, `column-order`, …).
pub(super) fn mutation_expected_properties(mutation: &Mutation) -> HashMap<String, Value> {
    let fields = match mutation {
        Mutation::Create { fields, .. } | Mutation::Update { fields, .. } => fields,
        _ => return HashMap::new(),
    };
    fields
        .iter()
        .filter(|(k, _)| !BLOCK_SQL_COLUMNS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Parse a `properties` column value into a flat map, handling the two
/// shapes Turso may return (raw JSON string or already-parsed Object).
pub(super) fn row_properties_to_map(props_val: &Value) -> HashMap<String, Value> {
    match props_val {
        Value::String(s) => serde_json::from_str::<HashMap<String, Value>>(s).unwrap_or_default(),
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => HashMap::new(),
    }
}
