//! `build_block_params` for the markdown flavors — mirrors
//! `holon_orgmode::block_params::build_block_params` (the canonical org one) so
//! file-originated markdown blocks produce the same create/update op shape as
//! org blocks. Kept local (not shared via `holon-orgmode`) so this crate does
//! not depend on the org sync stack; the field set is the format-neutral core.

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::ROUTING_DOC_URI_KEY;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::marks_to_json;
use holon_org_format::OrgBlockExt;

pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
) -> StorageEntity {
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(block.id.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    params.insert(
        ROUTING_DOC_URI_KEY.into(),
        Value::String(document_uri.to_string()),
    );
    params.insert("content".into(), Value::String(block.content.clone()));
    params.insert(
        "content_type".into(),
        Value::String(block.content_type.to_string()),
    );

    // Marks → JSON TEXT column; None omits (NULL). Dropping marks re-emits the
    // stripped label on any writeback and destroys the user's link syntax on
    // disk (ADR 0025 / links-ruling): marks are truth, never derived.
    if let Some(ref marks) = block.marks {
        params.insert("marks".into(), Value::String(marks_to_json(marks)));
    }

    let now = holon_api::clock::now_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".into(), Value::Integer(created));
    params.insert("updated_at".into(), Value::Integer(now));

    // Edge-typed fields — always emit (empty Vec clears stale junction rows).
    params.insert(
        "tags".into(),
        Value::Array(
            block
                .tags
                .iter()
                .map(|t| Value::String(t.clone()))
                .collect(),
        ),
    );
    params.insert("requires".into(), Value::Array(vec![]));
    params.insert("advice_suppressed".into(), Value::Array(vec![]));

    if let Some(task_state) = block.task_state() {
        params.insert("task_state".into(), Value::String(task_state.to_string()));
        params.insert(
            "task_state_category".into(),
            Value::String(task_state.category.as_str().to_string()),
        );
    }
    if let Some(priority) = block.priority() {
        params.insert("priority".into(), Value::Integer(priority.to_int() as i64));
    }
    if let Some(scheduled) = block.scheduled() {
        params.insert("scheduled".into(), Value::String(scheduled.to_string()));
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".into(), Value::String(deadline.to_string()));
    }

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
    }

    params
}
