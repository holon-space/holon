//! `set_field` is the THIRD production write leg into the `properties` bag,
//! beside `prepare_create` and `prepare_update`, and it patches the column in
//! place. These drive it against the real provider and read back through the
//! production read boundary.
//!
//! Not through the certification harness: that wiring registers
//! `SqlBlockOperations`, which offers a `set_field` to the `BlockCellRegistry`
//! first and returns `Ok` with no synchronous SQL write — so the leg under test
//! here is never reached there.

use super::*;

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
                properties TEXT,
                property_kinds TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await
        .expect("DDL");

    let provider = SqlOperationProvider::new(
        db_handle.clone(),
        "block_raw".to_string(),
        "block".to_string(),
        "block".to_string(),
    );
    let id = "kinds-block".to_string();
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id.clone()));
    params.insert("content".into(), Value::String("anchor".to_string()));
    provider
        .execute_operation(&EntityName::from("block"), "create", params)
        .await
        .expect("the anchor block must be creatable");

    // The write path promotes a bare id to its `block:` URI, so later
    // operations must name the STORED form or they update nothing and every
    // assertion below reads an untouched row.
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

/// Read one property back through the production boundary — the same
/// normalization every query and CDC row goes through.
async fn read_property(
    db_handle: &DbHandle,
    id: &str,
    key: &str,
) -> std::result::Result<Option<Value>, String> {
    let _ = id;
    let rows = db_handle
        .query(
            "SELECT properties, property_kinds FROM block_raw",
            HashMap::new(),
        )
        .await
        .map_err(|e| e.to_string())?;
    // The fixture holds exactly one block; the write path promotes the bare id
    // to its `block:` URI, so filtering on the id handed in would miss the row.
    assert_eq!(rows.len(), 1, "the fixture must hold exactly one block");
    let row = rows.into_iter().next().expect("just counted one");
    match row.get("properties") {
        Some(Value::Object(bag)) => Ok(bag.get(key).cloned()),
        other => panic!("the read boundary must yield an object bag, got {other:?}"),
    }
}

async fn set_field(provider: &SqlOperationProvider, id: &str, key: &str, value: Value) {
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(key.to_string()));
    params.insert("value".into(), value);
    provider
        .execute_operation(&EntityName::from("block"), "set_field", params)
        .await
        .expect("set_field must land");
}

async fn create_with(provider: &SqlOperationProvider, id: &str, key: &str, value: Value) {
    let mut params: holon_api::StorageEntity = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert(key.into(), value);
    provider
        .execute_operation(&EntityName::from("block"), "update", params)
        .await
        .expect("the kinded write must land");
}

/// D1 — the brick. Two ordinary writes must leave a READABLE row: a kind entry
/// that outlives the value it described makes every later read of that row
/// fail.
#[tokio::test]
async fn set_field_over_a_kinded_key_replaces_its_kind() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;
    create_with(
        &provider,
        &id,
        "Probe",
        Value::DateTime("2026-08-22T10:00:00Z".to_string()),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "Probe").await.expect("read"),
        Some(Value::DateTime("2026-08-22T10:00:00Z".to_string())),
        "the update leg must record the kind, or the overwrite below proves nothing"
    );

    set_field(
        &provider,
        &id,
        "Probe",
        Value::String("just a plain string".to_string()),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "Probe").await.expect(
            "the row must still be readable: a surviving date_time entry fails the whole read"
        ),
        Some(Value::String("just a plain string".to_string())),
    );
}

/// D2 — a removal through `set_field` takes the kind entry with the key.
#[tokio::test]
async fn set_field_removal_drops_the_kind_entry() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;
    create_with(
        &provider,
        &id,
        "Probe",
        Value::DateTime("2026-08-22T10:00:00Z".to_string()),
    )
    .await;

    set_field(&provider, &id, "Probe", Value::REMOVED).await;
    assert_eq!(
        read_property(&db_handle, &id, "Probe").await.expect("read"),
        None,
        "the removal must take the key out of the bag"
    );

    set_field(
        &provider,
        &id,
        "Probe",
        Value::String("plain now".to_string()),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "Probe")
            .await
            .expect("a kind entry that outlived its key would fail this read"),
        Some(Value::String("plain now".to_string())),
    );
}

/// D3 — `set_field` must RECORD a kind, not only clear one. Without this the
/// kind is silently lost on this leg while the profile claims it survives.
#[tokio::test]
async fn set_field_records_the_kind_it_writes() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;

    set_field(
        &provider,
        &id,
        "when",
        Value::DateTime("2026-08-22T10:00:00Z".to_string()),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "when").await.expect("read"),
        Some(Value::DateTime("2026-08-22T10:00:00Z".to_string())),
        "a DateTime through set_field must not read back as the String JSON kept"
    );

    set_field(&provider, &id, "doc", Value::Json(r#"{"a":1}"#.to_string())).await;
    assert_eq!(
        read_property(&db_handle, &id, "doc").await.expect("read"),
        Some(Value::Json(r#"{"a":1}"#.to_string())),
        "a Json document through set_field must not read back as an Object"
    );

    // The same spelling question, one kind over: an Array must land as a JSON
    // array, not as the TEXT of one.
    set_field(
        &provider,
        &id,
        "list",
        Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "list").await.expect("read"),
        Some(Value::Array(vec![Value::Integer(1), Value::Integer(2)])),
        "an Array through set_field must not read back as the string of its JSON"
    );
}

/// A declared type must not depend on WHICH leg wrote it. SQLite has no
/// boolean, so the literal spelling `1` made this leg store a number where the
/// create leg stores JSON `true` — two routes disagreeing about `boolean`,
/// which the certifier cannot contradict because it cannot drive this leg at
/// all.
#[tokio::test]
async fn set_field_round_trips_a_boolean() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;

    set_field(&provider, &id, "flag", Value::Boolean(true)).await;
    assert_eq!(
        read_property(&db_handle, &id, "flag").await.expect("read"),
        Some(Value::Boolean(true)),
        "a Boolean through set_field must not degrade to Integer(1)"
    );

    set_field(&provider, &id, "off", Value::Boolean(false)).await;
    assert_eq!(
        read_property(&db_handle, &id, "off").await.expect("read"),
        Some(Value::Boolean(false)),
        "false must not degrade to Integer(0), which also reads as a different kind"
    );
}

/// The kind map's empty spelling is NULL on every leg, so "no key carries a
/// non-evident kind" cannot be two different stored states.
#[tokio::test]
async fn an_emptied_kind_map_is_stored_as_null() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;
    set_field(
        &provider,
        &id,
        "when",
        Value::DateTime("2026-08-22T10:00:00Z".to_string()),
    )
    .await;
    set_field(&provider, &id, "when", Value::String("plain".to_string())).await;

    let rows = db_handle
        .query("SELECT property_kinds FROM block_raw", HashMap::new())
        .await
        .expect("read the kinds column");
    let stored = rows
        .into_iter()
        .next()
        .expect("the fixture's one block")
        .get("property_kinds")
        .cloned();
    assert!(
        matches!(stored, None | Some(Value::Null)),
        "the last kind entry going away must leave NULL, not an empty object: {stored:?}"
    );
}

/// `task_state` writes a `task_state_category` sidecar in the SAME statement.
/// It goes through the one bag writer too, so the pair still lands together.
#[tokio::test]
async fn the_task_state_sidecar_still_rides_the_one_bag_writer() {
    let (_backend, db_handle, provider, id) = provider_with_block().await;
    set_field(
        &provider,
        &id,
        "task_state",
        Value::String("DONE".to_string()),
    )
    .await;
    assert_eq!(
        read_property(&db_handle, &id, "task_state")
            .await
            .expect("read"),
        Some(Value::String("DONE".to_string())),
    );
    assert!(
        read_property(&db_handle, &id, "task_state_category")
            .await
            .expect("read")
            .is_some(),
        "the derived category sidecar must still be written with the keyword"
    );

    set_field(&provider, &id, "task_state", Value::REMOVED).await;
    assert_eq!(
        read_property(&db_handle, &id, "task_state")
            .await
            .expect("read"),
        None
    );
    assert_eq!(
        read_property(&db_handle, &id, "task_state_category")
            .await
            .expect("read"),
        None,
        "removing the keyword must remove its sidecar"
    );
}
