//! Unit tests for the per-column Rust diff guard in `prepare_update`.
//!
//! Each test covers one invariant from the verification plan (items 1-4).

use super::*;

const BLOCK_TABLE: &str = "block_raw";

async fn make_provider_with_block(
    content: &str,
    properties: Option<&str>,
    updated_at: i64,
) -> (
    crate::storage::turso::TursoBackend,
    DbHandle,
    SqlOperationProvider,
    String,
) {
    let (backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    db_handle
        .execute_ddl(
            "CREATE TABLE block_raw (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                content TEXT NOT NULL DEFAULT '',
                content_type TEXT NOT NULL DEFAULT 'text',
                properties TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await
        .expect("DDL");

    let id = "test-block-1".to_string();
    let props_sql = match properties {
        Some(p) => format!("'{}'", p.replace('\'', "''")),
        None => "NULL".to_string(),
    };
    let insert = format!(
        "INSERT INTO block_raw (id, content, properties, created_at, updated_at) \
         VALUES ('{id}', '{content}', {props_sql}, 1000, {updated_at})"
    );
    db_handle
        .execute(&insert, vec![])
        .await
        .expect("insert test block");

    let provider = SqlOperationProvider::new(
        db_handle.clone(),
        BLOCK_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
    );
    (backend, db_handle, provider, id)
}

/// Test 1: identity update returns None — no write, no event.
#[tokio::test]
async fn prepare_update_returns_none_on_identity_update() {
    let (_backend, _handle, provider, id) =
        make_provider_with_block("hello world", None, 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert("content".into(), Value::String("hello world".to_string()));

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update");
    assert!(
        result.is_none(),
        "identity update (same content) must return None, got Some"
    );
}

/// Test 2: timestamp-only update returns None (H2 guard).
///
/// `updated_at` is regenerated to `now()` on every call; if it's the ONLY
/// changed column, no real content changed, so we must not publish an event.
#[tokio::test]
async fn prepare_update_returns_none_for_timestamp_only_change() {
    let (_backend, _handle, provider, id) =
        make_provider_with_block("stable content", None, 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert(
        "content".into(),
        Value::String("stable content".to_string()),
    );
    // Caller provides an explicit updated_at that differs from the stored value.
    // This simulates the block_to_params regeneration-to-now pattern.
    params.insert("updated_at".into(), Value::Integer(99999));

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update");
    assert!(
        result.is_none(),
        "timestamp-only update must return None; got Some (H2 regression)"
    );
}

/// Test 3: update with one real content change returns Some with both the
/// content field AND updated_at in the SET clause.
#[tokio::test]
async fn prepare_update_returns_some_when_content_changed() {
    let (_backend, _handle, provider, id) =
        make_provider_with_block("old content", None, 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert("content".into(), Value::String("new content".to_string()));
    params.insert("updated_at".into(), Value::Integer(99999));

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp) — content changed");

    let sql = result
        .row_statements
        .iter()
        .chain(&result.edge_statements)
        .cloned()
        .collect::<Vec<_>>()
        .join(";");
    assert!(
        sql.contains("'new content'"),
        "SET clause must include new content value; SQL: {sql}"
    );
    assert!(
        sql.contains("updated_at"),
        "SET clause must include updated_at when content changed; SQL: {sql}"
    );
}

/// Test 4: non-canonical stored properties compared with canonical extra_props
/// returns None (no write) when the key-value sets are equal.
///
/// The stored value has keys in insertion order (b before a); the incoming
/// extra_props map has them in the opposite order. After canonicalisation
/// both produce the same JSON string — so the diff should be a no-op.
#[tokio::test]
async fn prepare_update_returns_none_for_non_canonical_stored_properties() {
    // Store properties with key order b, a (non-canonical).
    let stored_props = r#"{"b":1,"a":2}"#;
    let (_backend, _db, provider, id) =
        make_provider_with_block("x", Some(stored_props), 1000).await;

    // Submit an update with extra_props a=2, b=1 (same values, different order).
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    // Extra props are passed as individual Value entries that partition_params
    // routes to extra_props because they're not known SQL columns.
    params.insert("a".into(), Value::Integer(2));
    params.insert("b".into(), Value::Integer(1));

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update");
    assert!(
        result.is_none(),
        "canonical-equivalent properties update must return None; got Some (JSON ordering bug)"
    );
}

/// `Value::Null` extra-prop is a property-REMOVAL sentinel: the merged
/// properties JSON must no longer contain the key. Pins the `#+TODO:`
/// keyword-set-deleted-from-org-header path (org sync emits
/// `todo_keywords: Null` via `LiveDocumentManager::update_metadata`).
#[tokio::test]
async fn prepare_update_null_prop_removes_key_from_properties() {
    let stored_props = r#"{"keep":"v","todo_keywords":"[{\"keyword\":\"WIP\"}]"}"#;
    let (_backend, _db, provider, id) =
        make_provider_with_block("x", Some(stored_props), 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert("todo_keywords".into(), Value::Null);

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp) — property removal is a real change");

    let sql = result
        .row_statements
        .iter()
        .chain(&result.edge_statements)
        .cloned()
        .collect::<Vec<_>>()
        .join(";");
    assert!(
        !sql.contains("todo_keywords"),
        "removed key must not appear in merged properties JSON; SQL: {sql}"
    );
    assert!(
        sql.contains("keep"),
        "untouched keys must survive the merge; SQL: {sql}"
    );
}

/// Removing a key that doesn't exist is a no-op — the diff guard must
/// suppress the UPDATE entirely (no spurious CDC).
#[tokio::test]
async fn prepare_update_null_prop_for_absent_key_is_noop() {
    let stored_props = r#"{"keep":"v"}"#;
    let (_backend, _db, provider, id) =
        make_provider_with_block("x", Some(stored_props), 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert("todo_keywords".into(), Value::Null);

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update");
    assert!(
        result.is_none(),
        "removing an absent key must be a no-op; got Some"
    );
}
