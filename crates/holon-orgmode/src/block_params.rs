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
    // to the `block_tags`/`block_requires`/`advice_suppressed` junctions (see
    // schema_modules.rs::edge_fields). Always emit (even when empty) so an
    // empty Vec correctly clears stale junction rows on update, and so strict
    // row parsing downstream always sees all three columns.
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

    let arr: Vec<Value> = block
        .advice_suppressed
        .iter()
        .map(|r| Value::String(r.to_string()))
        .collect();
    params.insert("advice_suppressed".into(), Value::Array(arr));

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
        // `cycle_task_state` writes this sidecar in the same statement as
        // `task_state` (`sql_operation_provider.rs`'s `category_str_for_keyword`
        // pairing) so category-filtering queries can see the state without a
        // keyword-list join. The org parser already derived the category from
        // `#+TODO:` config (`TaskState::from_keyword_with_done_list`) — mirror
        // it here so file-originated tasks pair the same way, one source of
        // truth (`TaskState.category`) instead of two.
        params.insert(
            "task_state_category".into(),
            Value::String(task_state.category.as_str().to_string()),
        );
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
        // Same shape for `:ADVICE_SUPPRESSED:` (ADR 0021): typed edge field
        // already emitted as a `Value::Array` param above (routed to the
        // `advice_suppressed` junction); the drawer string must not leak into
        // `block.properties`.
        if k.eq_ignore_ascii_case("advice_suppressed") {
            continue;
        }
        params.insert(k.into(), Value::String(v));
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_org_file;

    /// Regression: org-ingested TODO/DONE blocks must carry BOTH `task_state`
    /// and its `task_state_category` sidecar in the params sent to
    /// create/update — otherwise category-filtering queries never see
    /// file-originated tasks (only ones cycled through the UI, which pairs
    /// them via `cycle_task_state`). The category is already derived by the
    /// parser (`TaskState::from_keyword_with_done_list` off `#+TODO:`
    /// config); this boundary must not drop it.
    #[test]
    fn ingested_todo_and_done_blocks_carry_task_state_category() {
        let org = "\
#+TODO: TODO | DONE

* TODO Buy milk
* DONE Ship it
";
        let parent_dir_id = EntityUri::no_parent();
        let path = std::path::Path::new("/vault/doc.org");
        let root = std::path::Path::new("/vault");
        let parsed = parse_org_file(path, org, &parent_dir_id, root).expect("parse org fixture");

        let headlines: Vec<&Block> = parsed
            .blocks
            .iter()
            .filter(|b| b.task_state().is_some())
            .collect();
        assert_eq!(
            headlines.len(),
            2,
            "expected exactly the TODO and DONE headlines, got {:?}",
            parsed.blocks
        );

        for block in headlines {
            let params = build_block_params(block, &parsed.document.id, &parsed.document.id);
            let task_state = params
                .get("task_state")
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| panic!("task_state missing from ingest params for {block:?}"));
            let category = params
                .get("task_state_category")
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| {
                    panic!("task_state_category missing from ingest params for {block:?}")
                });

            let expected_category = if task_state == "DONE" {
                "done"
            } else {
                "active"
            };
            assert_eq!(
                category, expected_category,
                "wrong task_state_category for keyword {task_state:?}"
            );
        }
    }
}
