//! `build_block_params` for plugin ingest — the format-neutral core, with no
//! org task/scheduling fields, which no plugin format has syntax for.

use holon_api::EntityUri;
use holon_api::ROUTING_DOC_URI_KEY;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;

/// The `block_raw` storage columns, as one set built once.
static BLOCK_STORAGE_COLUMNS: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| holon_api::schema::BLOCK.columns().into_iter().collect());

/// True for a property key that spells a `block_raw` STORAGE COLUMN.
pub(crate) fn names_block_storage_column(key: &str) -> bool {
    BLOCK_STORAGE_COLUMNS.contains(key)
}

/// `previous` is accepted to satisfy the adapter contract but has nothing to
/// act on: these params carry no user-authored property namespace, so no key
/// can go stale in the store.
pub(crate) fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
    previous: Option<&Block>,
) -> anyhow::Result<StorageEntity> {
    let _ = previous;
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

    let now = holon_api::clock::now_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".into(), Value::Integer(created));
    params.insert("updated_at".into(), Value::Integer(now));

    // Over the closed edge set so a newly added edge cannot be silently
    // omitted. A plugin format expresses no block-referencing syntax, so every
    // edge is emitted EMPTY — the file is authoritative, and absence of syntax
    // means the edge is gone, not merely unmentioned.
    for field in holon_api::EdgeField::ALL {
        params.insert(field.column().into(), Value::Array(Vec::new()));
    }

    // Properties are FLAT params: `partition_params` routes each one to a
    // column or the property bag.
    for (key, value) in &block.properties {
        // A hard refusal, not a debug_assert: a debug_assert is a no-op in
        // release, exactly where overwriting a row's real state would do its
        // damage unseen.
        if names_block_storage_column(key) {
            anyhow::bail!(
                "block {} carries property {key:?}, which names a `block_raw` storage column; \
                 emitting it as a param would overwrite the row's own state",
                block.id
            );
        }
        params.insert(key.as_str().into(), value.clone());
    }

    Ok(params)
}
