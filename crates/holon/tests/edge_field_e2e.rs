//! End-to-end test for Track 1A — edge-typed field abstraction.
//!
//! Verifies, against a real Turso DB, that the new third partition in
//! `SqlOperationProvider`:
//!   1. routes `Value::Array` payloads on a registered edge field through
//!      DELETE+INSERT against the junction table (NOT into the JSON
//!      properties blob);
//!   2. handles create / update / set_field / delete uniformly;
//!   3. relies on FK CASCADE so a parent-block delete drops incident
//!      junction rows.
//!
//! Mirrors H1's harness shape (see
//! `crates/holon/examples/turso_ivm_junction_gating_repro.rs`) — but here
//! the writes go through the production code path, not raw SQL.

use std::collections::HashMap;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::datasource::OperationProvider;
use holon::storage::schema_module::{EdgeFieldDescriptor, SchemaModule};
use holon::storage::schema_modules::BlockSchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::{EntityName, Value};

const ENTITY: &str = "block";
const TABLE: &str = "block_raw";

fn descriptor() -> EdgeFieldDescriptor {
    EdgeFieldDescriptor {
        entity: ENTITY.to_string(),
        field: "requires".to_string(),
        join_table: "block_requires".to_string(),
        source_col: "block_id".to_string(),
        target_col: "required_id".to_string(),
    }
}

async fn setup_schema(handle: &holon::storage::turso::DbHandle) {
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("enable FKs");
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                tags TEXT,
                content TEXT NOT NULL DEFAULT '',
                content_type TEXT NOT NULL DEFAULT 'text',
                properties TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await
        .expect("block_raw table");
    handle
        .execute_ddl(
            "CREATE TABLE block_requires (
                block_id TEXT NOT NULL,
                required_id TEXT NOT NULL,
                PRIMARY KEY (block_id, required_id),
                FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE,
                FOREIGN KEY (required_id) REFERENCES block_raw(id) ON DELETE CASCADE
            )",
        )
        .await
        .expect("block_requires table");
    // `find_document_uri` joins block_tags unconditionally; create it so
    // SqlOperationProvider can satisfy the page-resolution query even when
    // the test only exercises block_requires.
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (
                block_id TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (block_id, tag),
                FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
            )",
        )
        .await
        .expect("block_tags table");
}

async fn read_requires(handle: &holon::storage::turso::DbHandle, block_id: &str) -> Vec<String> {
    let sql = format!(
        "SELECT required_id FROM block_requires WHERE block_id = '{}' ORDER BY required_id",
        block_id.replace('\'', "''")
    );
    let rows = handle.query(&sql, HashMap::new()).await.expect("query");
    rows.into_iter()
        .filter_map(|r| {
            r.get("required_id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect()
}

async fn read_properties(handle: &holon::storage::turso::DbHandle, id: &str) -> Option<String> {
    let sql = format!(
        "SELECT properties FROM block_raw WHERE id = '{}'",
        id.replace('\'', "''")
    );
    let rows = handle.query(&sql, HashMap::new()).await.expect("query");
    let row = rows.into_iter().next()?;
    let v = row.get("properties")?;
    match v {
        holon_api::Value::Null => None,
        holon_api::Value::String(s) => Some(s.clone()),
        other => Some(format!("{:?}", other)),
    }
}

fn make_provider(handle: holon::storage::turso::DbHandle) -> SqlOperationProvider {
    SqlOperationProvider::with_edge_fields(
        handle,
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![descriptor()],
    )
}

fn create_params(id: &str, content: &str, requires: &[&str]) -> holon_api::StorageEntity {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("content".into(), Value::String(content.to_string()));
    if !requires.is_empty() {
        p.insert(
            "requires".into(),
            Value::Array(
                requires
                    .iter()
                    .map(|s| Value::String((*s).to_string()))
                    .collect(),
            ),
        );
    }
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_field_routes_through_junction_on_create_update_and_delete() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;

    let provider = Arc::new(make_provider(handle.clone()));
    let entity_name: EntityName = ENTITY.to_string().into();

    // --- pre-create the blocker rows so the FK is satisfied -------------
    for id in ["B", "C", "D"] {
        let mut p: holon_api::StorageEntity = HashMap::new();
        p.insert("id".into(), Value::String(id.to_string()));
        p.insert("content".into(), Value::String(id.to_string()));
        provider
            .execute_operation(&entity_name, "create", p)
            .await
            .expect("create blocker");
    }

    // --- create A with requires = [B, C] --------------------------------
    let create = create_params("A", "task A", &["B", "C"]);
    provider
        .execute_operation(&entity_name, "create", create)
        .await
        .expect("create A");

    let requires = read_requires(&handle, "A").await;
    assert_eq!(
        requires,
        vec!["B".to_string(), "C".to_string()],
        "junction must hold the requires edges after create"
    );

    let props = read_properties(&handle, "A").await.unwrap_or_default();
    assert!(
        !props.contains("requires"),
        "edge field MUST NOT leak into properties JSON; got: {props}"
    );

    // --- update A: requires = [C, D] (drop B, add D) --------------------
    let mut update: holon_api::StorageEntity = HashMap::new();
    update.insert("id".into(), Value::String("A".to_string()));
    update.insert(
        "requires".into(),
        Value::Array(vec![
            Value::String("C".to_string()),
            Value::String("D".to_string()),
        ]),
    );
    provider
        .execute_operation(&entity_name, "update", update)
        .await
        .expect("update A");

    let requires = read_requires(&handle, "A").await;
    assert_eq!(
        requires,
        vec!["C".to_string(), "D".to_string()],
        "update must replace the junction rows"
    );

    // --- set_field with empty Array clears the edges -------------------
    let mut clear: holon_api::StorageEntity = HashMap::new();
    clear.insert("id".into(), Value::String("A".to_string()));
    clear.insert("field".into(), Value::String("requires".to_string()));
    clear.insert("value".into(), Value::Array(Vec::new()));
    provider
        .execute_operation(&entity_name, "set_field", clear)
        .await
        .expect("set_field clear");

    let requires = read_requires(&handle, "A").await;
    assert!(
        requires.is_empty(),
        "set_field with empty Array must clear junction rows; got {requires:?}"
    );

    // --- set_field with a non-empty Array re-installs them -------------
    let mut reset: holon_api::StorageEntity = HashMap::new();
    reset.insert("id".into(), Value::String("A".to_string()));
    reset.insert("field".into(), Value::String("requires".to_string()));
    reset.insert(
        "value".into(),
        Value::Array(vec![Value::String("B".to_string())]),
    );
    provider
        .execute_operation(&entity_name, "set_field", reset)
        .await
        .expect("set_field reset");

    let requires = read_requires(&handle, "A").await;
    assert_eq!(requires, vec!["B".to_string()]);

    // --- FK CASCADE: deleting block B drops the junction row -----------
    let mut delete_b: holon_api::StorageEntity = HashMap::new();
    delete_b.insert("id".into(), Value::String("B".to_string()));
    provider
        .execute_operation(&entity_name, "delete", delete_b)
        .await
        .expect("delete B");

    let requires = read_requires(&handle, "A").await;
    assert!(
        requires.is_empty(),
        "FK ON DELETE CASCADE should drop A→B from the junction; got {requires:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn non_edge_array_param_still_panics_at_partition() {
    // Belt-and-suspenders: a Value::Array on a *non-registered* field falls
    // into the JSON-properties path. We don't want that path to silently
    // accept arrays (the H5 bug). Confirm the existing partition treats it
    // as an "extra prop" and serialises it through value_to_json (which
    // round-trips arrays correctly), NOT through `format!("{:?}", v)`.
    //
    // Why this matters: the old partition_params code silently emitted
    // debug-formatted garbage. With the new value_to_json helper an
    // unregistered array round-trips as JSON. That's the canary the H5
    // spike pointed at.
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;

    let provider = Arc::new(make_provider(handle.clone()));
    let entity_name: EntityName = ENTITY.to_string().into();

    let mut create: holon_api::StorageEntity = HashMap::new();
    create.insert("id".into(), Value::String("Z".to_string()));
    create.insert("content".into(), Value::String("Z".into()));
    create.insert(
        "labels".into(),
        Value::Array(vec![
            Value::String("alpha".to_string()),
            Value::String("beta".to_string()),
        ]),
    );
    provider
        .execute_operation(&entity_name, "create", create)
        .await
        .expect("create Z");

    // Turso parses the JSON properties column eagerly into Value::Object —
    // so we read the column and assert structurally rather than on a raw
    // string. The earlier (broken) code path stored Value::Array via
    // `format!("{:?}", v)`, which would produce a String containing
    // "Array([..])"; the fixed value_to_json round-trip emits a real
    // JSON array which Turso decodes into Value::Object{labels: Array(..)}.
    let row = handle
        .query(
            "SELECT properties FROM block_raw WHERE id = 'Z'",
            HashMap::new(),
        )
        .await
        .expect("read Z")
        .into_iter()
        .next()
        .expect("Z row");
    let props = row.get("properties").expect("properties column").clone();
    let labels = match &props {
        holon_api::Value::Object(map) => map.get("labels").cloned(),
        holon_api::Value::String(s) => {
            // If Turso returns the raw string, parse it ourselves.
            let parsed: serde_json::Value = serde_json::from_str(s).expect("valid JSON");
            Some(holon_api::Value::from_json_value(
                parsed
                    .as_object()
                    .and_then(|m| m.get("labels"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ))
        }
        other => panic!("unexpected properties type: {other:?}"),
    };
    let labels = labels.expect("labels key must exist in properties");
    let items = match labels {
        holon_api::Value::Array(a) => a,
        other => panic!(
            "unregistered array MUST round-trip as Value::Array (the H5 canary); got: {other:?}"
        ),
    };
    let strings: Vec<&str> = items
        .iter()
        .map(|v| match v {
            holon_api::Value::String(s) => s.as_str(),
            other => panic!("expected Value::String item, got {other:?}"),
        })
        .collect();
    assert_eq!(strings, vec!["alpha", "beta"]);
}

/// Setup using the production `BlockSchemaModule` (not inline DDL).
/// Creates the minimal `block_raw` table and then delegates to
/// `BlockSchemaModule::ensure_schema()` for the junction tables.
async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("enable FKs");
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                depth INTEGER NOT NULL DEFAULT 0,
                sort_key TEXT NOT NULL DEFAULT 'A0',
                content TEXT NOT NULL DEFAULT '',
                content_type TEXT NOT NULL DEFAULT 'text',
                source_language TEXT,
                source_name TEXT,
                tags TEXT,
                properties TEXT,
                marks TEXT,
                collapsed INTEGER NOT NULL DEFAULT 0,
                completed INTEGER NOT NULL DEFAULT 0,
                block_type TEXT NOT NULL DEFAULT 'text',
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                _change_origin TEXT
            )",
        )
        .await
        .expect("block_raw table");
    BlockSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockSchemaModule::ensure_schema");
}

async fn read_block_requires(
    handle: &holon::storage::turso::DbHandle,
    block_id: &str,
) -> Vec<String> {
    let sql = format!(
        "SELECT required_id FROM block_requires WHERE block_id = '{}' ORDER BY required_id",
        block_id.replace('\'', "''")
    );
    handle
        .query(&sql, HashMap::new())
        .await
        .expect("query block_requires")
        .into_iter()
        .filter_map(|r| {
            r.get("required_id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect()
}

async fn read_block_tags(handle: &holon::storage::turso::DbHandle, block_id: &str) -> Vec<String> {
    let sql = format!(
        "SELECT tag FROM block_tags WHERE block_id = '{}' ORDER BY tag",
        block_id.replace('\'', "''")
    );
    handle
        .query(&sql, HashMap::new())
        .await
        .expect("query block_tags")
        .into_iter()
        .filter_map(|r| {
            r.get("tag")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Track 1C: BlockSchemaModule creates the production junction tables and
/// declares the correct descriptors so SqlOperationProvider routes
/// `requires` → block_requires and `tags` → block_tags.
#[tokio::test(flavor = "multi_thread")]
async fn block_schema_module_creates_junction_tables_and_wires_edge_fields() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_production_schema(&handle).await;

    let descriptors = BlockSchemaModule.edge_fields();
    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        "block_raw".to_string(),
        "block".to_string(),
        "block".to_string(),
        descriptors,
    ));
    let entity_name: EntityName = "block".to_string().into();

    // pre-create the required blocks
    for id in ["X", "Y"] {
        let mut p: holon_api::StorageEntity = HashMap::new();
        p.insert("id".into(), Value::String(id.to_string()));
        p.insert("content".into(), Value::String(id.to_string()));
        provider
            .execute_operation(&entity_name, "create", p)
            .await
            .expect("create required");
    }

    // create task A with requires = [X, Y] and tags = [work, urgent]
    let mut create: holon_api::StorageEntity = HashMap::new();
    create.insert("id".into(), Value::String("A".to_string()));
    create.insert("content".into(), Value::String("task A".to_string()));
    create.insert(
        "requires".into(),
        Value::Array(vec![
            Value::String("X".to_string()),
            Value::String("Y".to_string()),
        ]),
    );
    create.insert(
        "tags".into(),
        Value::Array(vec![
            Value::String("urgent".to_string()),
            Value::String("work".to_string()),
        ]),
    );
    provider
        .execute_operation(&entity_name, "create", create)
        .await
        .expect("create A");

    let requires = read_block_requires(&handle, "A").await;
    assert_eq!(
        requires,
        vec!["X".to_string(), "Y".to_string()],
        "requires must land in block_requires"
    );

    let tags = read_block_tags(&handle, "A").await;
    assert_eq!(
        tags,
        vec!["urgent".to_string(), "work".to_string()],
        "tags must land in block_tags"
    );

    // verify neither leaked into properties JSON
    let props = read_properties(&handle, "A").await.unwrap_or_default();
    assert!(
        !props.contains("requires"),
        "requires must not bleed into properties; got: {props}"
    );
    assert!(
        !props.contains("tags"),
        "tags must not bleed into properties; got: {props}"
    );
}

// Empty-array on create is a no-op (no blockers to install); ensure no
// junction rows appear and properties JSON stays clean.
#[tokio::test(flavor = "multi_thread")]
async fn edge_field_create_with_empty_array_writes_no_junction_rows() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;

    let provider = Arc::new(make_provider(handle.clone()));
    let entity_name: EntityName = ENTITY.to_string().into();

    let mut create: holon_api::StorageEntity = HashMap::new();
    create.insert("id".into(), Value::String("X".to_string()));
    create.insert("content".into(), Value::String("X".into()));
    create.insert("requires".into(), Value::Array(Vec::new()));
    provider
        .execute_operation(&entity_name, "create", create)
        .await
        .expect("create X");

    let requires = read_requires(&handle, "X").await;
    assert!(requires.is_empty());
    let props = read_properties(&handle, "X").await.unwrap_or_default();
    assert!(
        !props.contains("requires"),
        "empty edge field must not bleed into properties JSON; got: {props}"
    );
}
