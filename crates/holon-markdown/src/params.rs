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

/// Whether an edit from `a` to `b` changes anything [`build_block_params`]
/// carries, so the ingest must emit an update. The priority cookie, planning
/// lines and `key:: value` lines are stripped from `content`, so each is
/// compared on its own.
pub fn content_differs(a: &Block, b: &Block) -> bool {
    a.content != b.content
        || a.marks != b.marks
        || a.tags != b.tags
        || a.task_state() != b.task_state()
        || a.priority() != b.priority()
        || a.scheduled() != b.scheduled()
        || a.deadline() != b.deadline()
        || a.drawer_properties() != b.drawer_properties()
}

/// `previous` is the block as the file previously declared it. A task marker,
/// priority, planning line or property `previous` carried and `block` no
/// longer does is emitted as `Value::REMOVED`, so the store drops what the
/// file dropped. `None` for a create.
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
    previous: Option<&Block>,
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

    // Edge-typed fields, over the closed set so a new one cannot be omitted.
    // Markdown expresses tags and nothing else, so every block-referencing edge
    // is emitted EMPTY: the file is authoritative, and an absent syntax means
    // the edge is gone, not merely unmentioned.
    for field in holon_api::EdgeField::ALL {
        let value = match field {
            holon_api::EdgeField::Tags => field.param_value(block),
            _ => Value::Array(Vec::new()),
        };
        params.insert(field.column().into(), value);
    }

    if let Some(task_state) = block.task_state() {
        params.insert("task_state".into(), Value::String(task_state.to_string()));
        params.insert(
            "task_state_category".into(),
            Value::String(task_state.category.as_str().to_string()),
        );
    } else if previous.is_some_and(|p| p.task_state().is_some()) {
        params.insert("task_state".into(), Value::REMOVED);
        params.insert("task_state_category".into(), Value::REMOVED);
    }
    if let Some(priority) = block.priority() {
        params.insert("priority".into(), Value::Integer(priority.rank() as i64));
    }
    if let Some(scheduled) = block.scheduled() {
        params.insert("scheduled".into(), Value::String(scheduled.to_string()));
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".into(), Value::String(deadline.to_string()));
    }

    for (key, value) in block.drawer_properties() {
        if is_storage_column(&key) {
            tracing::warn!(
                block = %block.id,
                key = %key,
                "markdown property names a `block_raw` storage column and cannot be stored as a \
                 property — dropping it from this block's ingest params. Rename the property."
            );
            continue;
        }
        params.insert(key.into(), Value::String(value));
    }

    if let Some(previous) = previous {
        let dropped_properties = previous
            .drawer_properties()
            .into_keys()
            .filter(|key| !is_storage_column(key));
        let dropped_fields = ["priority", "scheduled", "deadline"]
            .into_iter()
            .filter(|key| previous.get_property(key).is_some())
            .map(String::from);
        for key in dropped_properties.chain(dropped_fields) {
            if !params.contains_key(key.as_str()) {
                params.insert(key.into(), Value::REMOVED);
            }
        }
    }

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
    }

    params
}

/// A property named after a `block_raw` column would overwrite that column:
/// `SqlOperationProvider::partition_params` routes such a param to the column.
fn is_storage_column(key: &str) -> bool {
    holon_api::schema::BLOCK.columns().contains(&key)
}
