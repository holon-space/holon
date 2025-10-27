use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::EntityUri;
use holon_api::Value;
use std::collections::HashMap;

use crate::models::OrgBlockExt;

/// Build command parameters for a block create/update operation.
///
/// Converts a parsed `Block` into a flat `HashMap<String, Value>` suitable
/// for passing to `OperationProvider::execute_operation` (create/update).
///
/// The `document_uri` is inserted under `ROUTING_DOC_URI_KEY` so the
/// `OrgSyncController` can route the operation to the correct document
/// regardless of where `parent_id` points.
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(block.id.to_string()));
    params.insert(
        "parent_id".to_string(),
        Value::String(parent_id.to_string()),
    );
    // Routing metadata: tells OrgSyncController which document this block
    // belongs to, even when parent_id is another block (not a document).
    params.insert(
        holon::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
        Value::String(document_uri.to_string()),
    );
    params.insert("content".to_string(), Value::String(block.content.clone()));
    params.insert(
        "content_type".to_string(),
        Value::String(block.content_type.to_string()),
    );

    // Timestamps must be provided explicitly as integers (millis).
    // The blocks table DDL has `DEFAULT (datetime('now'))` which produces TEXT,
    // but Block::from_entity expects i64. Always provide integer timestamps
    // to avoid this mismatch.
    let now = chrono::Utc::now().timestamp_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".to_string(), Value::Integer(created));
    params.insert("updated_at".to_string(), Value::Integer(now));

    if let Some(ref name) = block.name {
        params.insert("name".to_string(), Value::String(name.clone()));
    }

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert(
                "source_language".to_string(),
                Value::String(lang.to_string()),
            );
        }
        if let Some(ref name) = block.source_name {
            params.insert("source_name".to_string(), Value::String(name.clone()));
        }
        let header_args = block.get_source_header_args();
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                params.insert("source_header_args".to_string(), Value::String(json));
            }
        }
    }

    if let Some(task_state) = block.task_state() {
        params.insert(
            "task_state".to_string(),
            Value::String(task_state.to_string()),
        );
    }
    if let Some(priority) = block.priority() {
        params.insert(
            "priority".to_string(),
            Value::Integer(priority.to_int() as i64),
        );
    }
    let tags = block.tags();
    if !tags.is_empty() {
        params.insert("tags".to_string(), Value::String(tags.to_csv()));
    }
    if let Some(scheduled) = block.scheduled() {
        params.insert(
            "scheduled".to_string(),
            Value::String(scheduled.to_string()),
        );
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".to_string(), Value::String(deadline.to_string()));
    }

    params.insert("sequence".to_string(), Value::Integer(block.sequence()));

    // Without this, every block reaches SQL with the default `sort_key='a0'`
    // because `build_block_params` is the choke point for OrgSyncController
    // CREATE / UPDATE batches. Sibling collisions on `'a0'` then break
    // `BlockOperations::get_prev_sibling` (filter `sort_key < block.sort_key`
    // is empty) and `gen_key_between` for `move_block` / `outdent`.
    params.insert(
        "sort_key".to_string(),
        Value::String(block.sort_key.clone()),
    );

    // Include org drawer properties (flat in block.properties)
    let id = block
        .get_block_id()
        .unwrap_or_else(|| block.id.id().to_string());
    params.insert("ID".to_string(), Value::String(id));

    for (k, v) in block.drawer_properties() {
        params.insert(k, Value::String(v));
    }

    params
}
