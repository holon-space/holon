use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::ContentType;

use crate::models::OrgBlockExt;

/// Build command parameters for a block create/update operation.
///
/// Converts a parsed `Block` into a flat `StorageEntity` suitable
/// for passing to `OperationProvider::execute_operation` (create/update).
///
/// The `document_uri` is inserted under `ROUTING_DOC_URI_KEY` as the
/// param-side routing hint. `SqlOperationProvider` lifts the value onto
/// the typed `Event::routing_doc_uri` field at its boundary; the consumer
/// (`FileSyncController`) reads the typed field, so it can route the
/// operation to the correct document regardless of where `parent_id`
/// points.
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
) -> holon_api::StorageEntity {
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(block.id.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    // Routing metadata: tells FileSyncController which document this block
    // belongs to, even when parent_id is another block (not a document).
    params.insert(
        holon_api::ROUTING_DOC_URI_KEY.into(),
        Value::String(document_uri.to_string()),
    );
    params.insert("content".into(), Value::String(block.content.clone()));
    params.insert(
        "content_type".into(),
        Value::String(block.content_type.to_string()),
    );

    // Timestamps must be provided explicitly as integers (millis).
    // The blocks table DDL has `DEFAULT (datetime('now'))` which produces TEXT,
    // but Block::from_entity expects i64. Always provide integer timestamps
    // to avoid this mismatch.
    let now = holon_api::clock::now_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".into(), Value::Integer(created));
    params.insert("updated_at".into(), Value::Integer(now));

    // Edge-typed fields — `SqlOperationProvider`'s edge partition routes these
    // to the `block_tags`/`block_requires` junctions (see
    // schema_modules.rs::edge_fields). Always emit (even when empty) so an
    // empty Vec correctly clears stale junction rows on update, and so strict
    // row parsing downstream always sees both columns.
    let arr: Vec<Value> = block
        .tags
        .iter()
        .map(|t| Value::String(t.clone()))
        .collect();
    params.insert("tags".into(), Value::Array(arr));

    let arr: Vec<Value> = block
        .requires
        .iter()
        .map(|r| Value::String(r.to_string()))
        .collect();
    params.insert("requires".into(), Value::Array(arr));

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
        if let Some(ref name) = block.source_name {
            params.insert("source_name".into(), Value::String(name.clone()));
        }
        let header_args = block.get_source_header_args();
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                params.insert("source_header_args".into(), Value::String(json));
            }
        }
    }

    if let Some(task_state) = block.task_state() {
        params.insert("task_state".into(), Value::String(task_state.to_string()));
    }
    if let Some(priority) = block.priority() {
        params.insert("priority".into(), Value::Integer(priority.to_int() as i64));
    }
    // Tags are already serialized into the `tags` JSON-array param above
    // (lines 53-57); the legacy CSV-via-properties shape is gone. Skip the
    // OrgBlockExt::tags() shim here so we don't overwrite the JSON list with
    // a comma-separated string.
    if let Some(scheduled) = block.scheduled() {
        params.insert("scheduled".into(), Value::String(scheduled.to_string()));
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".into(), Value::String(deadline.to_string()));
    }

    params.insert("sequence".into(), Value::Integer(block.sequence()));

    // sort_key is intentionally NOT emitted here. The org parser's
    // `gen_n_keys` value used to land in the sink via this map and competed
    // with the consolidator's auto-assigned order key — two generators in
    // disjoint string spaces, producing the seed=42 SplitBlock ordering
    // panic (devlog 2026-05-14). The single authoritative order writer is
    // the consolidator's outbound projection, which materializes its order
    // key into the sink's `sort_key` column. Position intent enters the
    // system via `after_block_id` (lifted to `Event::position_after_block_id`
    // at the provider boundary) and drives the consolidator's move op.

    // Include org drawer properties (flat in block.properties)
    let id = block
        .get_block_id()
        .unwrap_or_else(|| block.id.id().to_string());
    params.insert("ID".into(), Value::String(id));

    for (k, v) in block.drawer_properties() {
        // `drawer_properties()` emits `REQUIRES` for org *rendering* (the
        // org-edna dependency drawer). Here it must be skipped: `requires` is
        // an edge field already emitted as the typed `Value::Array` param above
        // (routed to the `block_requires` junction). Re-inserting it as a flat
        // string property would pollute `block.properties` with a stray
        // uppercase `REQUIRES` key that the reference model never has.
        if k.eq_ignore_ascii_case("requires") {
            continue;
        }
        params.insert(k.into(), Value::String(v));
    }

    params
}
