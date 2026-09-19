//! Invariant 15 (docs/Architecture/Model.md): a dispatched operation targets
//! an existing subject, and the write authority is what enforces it.
//!
//! The Loro authority already refuses a `set_field` whose block it cannot find
//! (`LoroBlockOperations::set_field` → `find_doc_for_block`). These pin the SQL
//! single-op arm to the same answer: an UPDATE that matches no row is a
//! refusal, not a success. Every arm of the `set_field` match that issues an
//! UPDATE is driven here — column, property bag, `parent_id` (the transaction
//! leg) and the rich-content Object leg — because each executes its write
//! through a different call and an assert on one says nothing about the rest.

use super::*;

const GHOST: &str = "block:ghost";

async fn provider_with_block() -> (
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
                marks TEXT,
                properties TEXT,
                property_kinds TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await
        .expect("DDL");
    // The rich-content leg re-derives this junction AFTER its UPDATE. Without
    // the table its red would be a missing-table error rather than the silent
    // success under test.
    db_handle
        .execute_ddl(
            "CREATE TABLE block_links (
                source_block_id TEXT NOT NULL,
                target TEXT NOT NULL,
                kind TEXT NOT NULL,
                resolved_id TEXT,
                PRIMARY KEY (source_block_id, target, kind)
            )",
        )
        .await
        .expect("block_links DDL");

    let provider = SqlOperationProvider::new(
        db_handle.clone(),
        "block_raw".to_string(),
        "block".to_string(),
        "block".to_string(),
    );
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String("block:anchor".to_string()));
    params.insert("content".into(), Value::String("anchor".to_string()));
    provider
        .execute_operation(&EntityName::from("block"), "create", params)
        .await
        .expect("the anchor block must be creatable");

    // Read the id back rather than reusing the literal: the assertions below
    // must address the row as it is STORED, or the positive control updates
    // nothing and the whole file passes vacuously.
    let stored_id = db_handle
        .query("SELECT id FROM block_raw", HashMap::new())
        .await
        .expect("read the anchor back")
        .into_iter()
        .next()
        .and_then(|r| r.get("id").and_then(|v| v.as_string().map(str::to_string)))
        .expect("the anchor row must exist");
    (backend, db_handle, provider, stored_id)
}

async fn set_field(
    provider: &SqlOperationProvider,
    id: &str,
    field: &str,
    value: Value,
) -> Result<OperationResult> {
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), value);
    provider
        .execute_operation(&EntityName::from("block"), "set_field", params)
        .await
}

/// The refusal has to say WHICH id and WHICH field, or a caller holding a stale
/// handle cannot tell which of its writes was the one that fell through.
fn assert_names_subject_and_field(err: &str, field: &str) {
    assert!(
        err.contains(GHOST) && err.contains(field),
        "the refusal must name the missing subject and the field, got: {err}"
    );
}

#[tokio::test]
async fn set_field_on_a_missing_subject_is_refused_for_a_column() {
    let (_backend, _db, provider, _anchor) = provider_with_block().await;
    let err = set_field(
        &provider,
        GHOST,
        "content",
        Value::String("writes into the void".to_string()),
    )
    .await
    .expect_err("a content write to a block nothing holds must not report success");
    assert_names_subject_and_field(&err.to_string(), "content");
}

#[tokio::test]
async fn set_field_on_a_missing_subject_is_refused_for_a_property() {
    let (_backend, _db, provider, _anchor) = provider_with_block().await;
    let err = set_field(
        &provider,
        GHOST,
        "task_state",
        Value::String("TODO".to_string()),
    )
    .await
    .expect_err("a property-bag write to a block nothing holds must not report success");
    assert_names_subject_and_field(&err.to_string(), "task_state");
}

/// `parent_id` is the one field whose UPDATE runs inside a transaction (the
/// deferred block FK is checked at COMMIT), so it reads its changed-row count
/// from a different handle method than every other field.
#[tokio::test]
async fn set_field_on_a_missing_subject_is_refused_for_parent_id() {
    let (_backend, _db, provider, anchor) = provider_with_block().await;
    let err = set_field(&provider, GHOST, "parent_id", Value::String(anchor))
        .await
        .expect_err("a reparent of a block nothing holds must not report success");
    assert_names_subject_and_field(&err.to_string(), "parent_id");
}

/// The rich-content leg (`content` carrying an Object) is reached by undo/redo
/// replay, whose subject can have been deleted since the entry was recorded.
#[tokio::test]
async fn set_field_on_a_missing_subject_is_refused_for_rich_content() {
    let (_backend, _db, provider, _anchor) = provider_with_block().await;
    let mut obj = HashMap::new();
    obj.insert("text".to_string(), Value::String("restored".to_string()));
    obj.insert("marks".to_string(), Value::Null);
    let err = set_field(&provider, GHOST, "content", Value::Object(obj))
        .await
        .expect_err("a rich-content restore of a block nothing holds must not report success");
    assert_names_subject_and_field(&err.to_string(), "content");
}

/// The assert rests on the driver counting rows MATCHED, not rows whose bytes
/// changed. Writing the SAME value twice is the case where those two readings
/// diverge: under a changed-bytes count the second write reports zero and the
/// assert would refuse an ordinary re-save — every keystroke that retypes a
/// character, every idempotent replay of an operation. Nothing else in the
/// tree pins which counting Turso does, so this does.
#[tokio::test]
async fn set_field_to_the_same_value_twice_still_lands() {
    let (_backend, _db, provider, id) = provider_with_block().await;
    let mut child: holon_api::StorageEntity = holon_api::StorageEntity::new();
    child.insert("id".into(), Value::String("block:child".to_string()));
    child.insert("content".into(), Value::String("child".to_string()));
    provider
        .execute_operation(&EntityName::from("block"), "create", child)
        .await
        .expect("the child block must be creatable");

    for round in ["first", "second"] {
        set_field(
            &provider,
            &id,
            "content",
            Value::String("unchanged".to_string()),
        )
        .await
        .unwrap_or_else(|e| panic!("{round} column write of an identical value must land: {e}"));
        set_field(
            &provider,
            &id,
            "task_state",
            Value::String("TODO".to_string()),
        )
        .await
        .unwrap_or_else(|e| panic!("{round} property write of an identical value must land: {e}"));
        set_field(
            &provider,
            "block:child",
            "parent_id",
            Value::String(id.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("{round} reparent to the same parent must land: {e}"));
    }
}

/// The positive control: the assert refuses a missing subject and nothing else.
/// Without it, an assert that refused EVERY `set_field` would pass the three
/// tests above.
#[tokio::test]
async fn set_field_on_an_existing_subject_still_lands() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;
    set_field(
        &provider,
        &id,
        "content",
        Value::String("rewritten".to_string()),
    )
    .await
    .expect("a write to a row that exists must land");
    set_field(
        &provider,
        &id,
        "task_state",
        Value::String("TODO".to_string()),
    )
    .await
    .expect("a property write to a row that exists must land");

    let content = db_handle
        .query("SELECT content FROM block_raw", HashMap::new())
        .await
        .expect("read back")
        .into_iter()
        .next()
        .and_then(|r| {
            r.get("content")
                .and_then(|v| v.as_string().map(str::to_string))
        })
        .expect("the anchor row must exist");
    assert_eq!(content, "rewritten");
}
