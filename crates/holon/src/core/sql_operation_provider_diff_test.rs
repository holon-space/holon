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
        "INSERT INTO block_raw (id, content, properties, created_at, updated_at) VALUES ('{id}', \
         '{content}', {props_sql}, 1000, {updated_at})"
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

/// The merged `properties` SQL for an update that submits a whole `properties`
/// BAG.
///
/// The bag is the route that reaches the blob's read-parse: `partition_params`
/// captures the `properties` param and parses each member into `extra_props`
/// (`sql_operation_provider.rs:595`). A test that instead passes loose
/// key/value params never touches that parse — it merges raw JSON — which is
/// why both tests below go through this helper.
async fn merged_properties_sql(stored_props: &str, submitted_bag: &str) -> String {
    let (_backend, _db, provider, id) =
        make_provider_with_block("x", Some(stored_props), 1000).await;

    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id));
    params.insert("properties".into(), Value::String(submitted_bag.into()));

    let result = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp) — the submitted bag is a real change");
    result
        .row_statements
        .iter()
        .chain(&result.edge_statements)
        .cloned()
        .collect::<Vec<_>>()
        .join(";")
}

/// A stored array or object survives the read-merge-write round with its KIND
/// intact.
///
/// This is the whole point of giving the blob boundary ONE parser: the merge
/// leg used to stringify these two shapes while every other reader of the same
/// blob kept them structured, so what a caller saw depended on which door it
/// came through. Drives the production `prepare_update` path — reverting the
/// merge leg to its old inline closure reds this test.
#[tokio::test]
async fn stored_containers_keep_their_kind_through_the_merge_leg() {
    let sql = merged_properties_sql(r#"{"keep":"v"}"#, r#"{"arr":[1,2],"obj":{"a":1}}"#).await;

    assert!(
        sql.contains(r#""arr":[1,2]"#),
        "a stored array must merge back as an array, not as the text \"[1,2]\"; SQL: {sql}"
    );
    assert!(
        sql.contains(r#""obj":{"a":1}"#),
        "a stored object must merge back as an object, not as text; SQL: {sql}"
    );
    assert!(
        sql.contains(r#""keep":"v""#),
        "untouched keys must survive the merge; SQL: {sql}"
    );
}

/// A stored JSON `null` keeps its key and its base serialization.
///
/// The hazard this pins is a silent DELETE: `Value::Null` is the property-
/// REMOVAL sentinel on both write legs, so parsing a stored null into it would
/// make merely READING the blob erase the key. `value_to_json` can put a
/// top-level null there (`Value::Float(NaN)` and `Value::Json("null")` both map
/// to one), and the delete/undo chain reads it straight back — so this is
/// reachable, not hypothetical.
///
/// Asserting base's exact serialization, the string `"null"`, is deliberate:
/// what null MEANS in the value space is open decision D27, and this leg must
/// not pre-decide it.
#[tokio::test]
async fn a_submitted_json_null_is_not_read_as_the_removal_sentinel() {
    let sql =
        merged_properties_sql(r#"{"k":"stored","keep":"v"}"#, r#"{"k":null,"other":"x"}"#).await;

    assert!(
        sql.contains(r#""k":"null""#),
        "a JSON null in the submitted bag must merge back as the string \"null\" (base behaviour, \
         D27 pending); SQL: {sql}"
    );
    assert!(
        !sql.contains(r#""k":"stored""#) || sql.contains(r#""k":"null""#),
        "the key must not silently keep AND lose its value; SQL: {sql}"
    );
    assert!(
        sql.contains(r#""keep":"v""#),
        "untouched keys must survive the merge; SQL: {sql}"
    );
}
