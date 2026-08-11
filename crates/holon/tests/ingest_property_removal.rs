//! Ingest is AUTHORITATIVE for a block's user-visible property set.
//!
//! A property key the org file no longer declares must not survive in the
//! store. The store-side merge has always had a removal sentinel
//! (`Value::Null` at a top-level params key, honoured by
//! `SqlOperationProvider::prepare_update`), but the ingest params builder never
//! EMITTED it — an insert-only merge kept every stale key alive, so renaming
//! `:leads-to:` to `:contributes-to:` in the vault left the store carrying
//! BOTH.
//!
//! Store-managed `_`-prefixed system keys are the exception: they never appear
//! in a file (the org writer erases them on write-back, see
//! docs/Reference/CompassConventions.md), so "absent from the file" says
//! nothing about them and ingest must preserve them.
//!
//! This drives the REAL org adapter (parse → `build_block_params`) into the
//! REAL SqlOnly ingest sink (`SqlBlockOperations::apply_ingest_batch`), and
//! reads the merged `properties` column back out of the store.
//!
//! @pbt kind harness
//! @pbt covers ingest-property-removal — a drawer key dropped from the file is
//!   dropped from the store, and `_`-prefixed system keys are not

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::OperationProvider;
use holon_core::block_ordering::BlockOrdering;
use holon_core::file_format::FileFormatAdapter;
use holon_core::storage::types::StorageEntity;
use holon_orgmode::file_format::OrgFormatAdapter;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The production SqlOnly block wiring (as `undo_inverse_wave1`), plus a handle
/// on the `SqlBlockOperations` so the test can drive the ingest sink directly.
async fn ingest_sink() -> (Arc<BackendEngine>, Arc<SqlBlockOperations>) {
    let captured: Arc<Mutex<Option<Arc<SqlBlockOperations>>>> = Arc::new(Mutex::new(None));
    let sink_slot = Arc::clone(&captured);
    let engine = create_test_engine_with_providers(":memory:".into(), move |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(move |backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread test
                    // runtime; block_in_place makes the bridge deadlock-free.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                let ops = Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)));
                *sink_slot.lock().expect("sink slot") = Some(Arc::clone(&ops));
                ops as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block provider");
    let sink = captured
        .lock()
        .expect("sink slot")
        .clone()
        .expect("the block-operations factory ran and published its sink");
    (engine, sink)
}

/// Parse one org file and hand its blocks to the ingest sink, exactly as
/// `FileSyncController`'s update pass does: `build_block_params` against the
/// PREVIOUS parse of the same file, then one `apply_ingest_batch`.
///
/// Returns the parse so the caller can feed it back as `previous`.
async fn ingest(
    sink: &SqlBlockOperations,
    source: &str,
    previous: Option<&holon_core::file_format::FileFormatParseResult>,
) -> holon_core::file_format::FileFormatParseResult {
    let adapter = OrgFormatAdapter::new();
    let root = PathBuf::from("/vault");
    let path = root.join("doc.org");
    let parsed = adapter
        .parse(&path, source, &EntityUri::no_parent(), &root)
        .expect("parse org source");

    let doc_uri = parsed.document.id.clone();
    let mut ops: Vec<(String, StorageEntity)> = Vec::new();
    // The document is a block too, and every headline's `parent_id` FK points
    // at it — `FileSyncController` materialises it before the block diff runs.
    ops.push((
        if previous.is_some() {
            "update"
        } else {
            "create"
        }
        .to_string(),
        ingest_params(
            &adapter,
            &parsed.document,
            &EntityUri::no_parent(),
            &doc_uri,
            previous.map(|p| &p.document),
        ),
    ));
    for block in &parsed.blocks {
        let prior = previous.and_then(|p| p.blocks.iter().find(|b| b.id == block.id));
        let params = ingest_params(&adapter, block, &doc_uri, &doc_uri, prior);
        let op = if prior.is_some() { "update" } else { "create" };
        ops.push((op.to_string(), params));
    }
    sink.apply_ingest_batch(ops).await.expect("ingest batch");
    parsed
}

/// The ONE params-building call the production ingest pass makes. Isolated so
/// the rung drives the same code path `FileSyncController` does.
fn ingest_params(
    adapter: &OrgFormatAdapter,
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
    previous: Option<&Block>,
) -> StorageEntity {
    adapter.build_block_params(block, parent_id, document_uri, previous)
}

/// The store id of the `p0` headline in a parse.
fn headline_id(parsed: &holon_core::file_format::FileFormatParseResult) -> String {
    parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "p0")
        .expect("the parse carries the `p0` headline")
        .id
        .to_string()
}

/// The block's stored `properties` JSON, decoded. `properties` is a jsonb
/// column: it comes back as `Value::Json`/`Value::Object`, NEVER a plain
/// `Value::String`, so a naive `as_string()` reader would collapse every state
/// to `None` and pass vacuously.
async fn stored_properties(
    engine: &BackendEngine,
    id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT properties FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("properties query");
    let cell = rows
        .first()
        .unwrap_or_else(|| panic!("no store row for {id}"))
        .get("properties")
        .cloned()
        .unwrap_or_else(|| panic!("no properties column for {id}"));
    match cell {
        Value::Null => serde_json::Map::new(),
        Value::String(s) if s.is_empty() => serde_json::Map::new(),
        Value::String(s) => serde_json::from_str(&s)
            .unwrap_or_else(|e| panic!("properties for {id} is not JSON ({e}): {s}")),
        other => {
            let json: serde_json::Value = other.into();
            match json {
                serde_json::Value::Object(m) => m,
                serde_json::Value::String(s) => serde_json::from_str(&s)
                    .unwrap_or_else(|e| panic!("properties for {id} is not JSON ({e}): {s}")),
                other => panic!("properties for {id} decoded to a non-object: {other}"),
            }
        }
    }
}

/// A block whose drawer declares `leads-to`, plus a store-managed `_sys` key
/// that no file ever carries.
const V1: &str = "\
#+ID: doc-props
* Problem
:PROPERTIES:
:ID: p0
:compass: problem
:leads-to: m1
:END:
";

/// The SAME block after the author renamed `leads-to` to `contributes-to`.
const V2: &str = "\
#+ID: doc-props
* Problem
:PROPERTIES:
:ID: p0
:compass: problem
:contributes-to: m1
:END:
";

/// A store-managed system key, seeded directly (it is unrepresentable in a
/// file — the org writer erases `_`-prefixed keys on write-back).
async fn seed_system_key(engine: &BackendEngine, id: &str, key: &str, value: &str) {
    let mut props = stored_properties(engine, id).await;
    props.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    let json = serde_json::to_string(&props).expect("Map→JSON cannot fail");
    engine
        .db_handle()
        .execute(
            &format!(
                "UPDATE {BLOCK_WRITE_TABLE} SET properties = '{}' WHERE id = '{}'",
                json.replace('\'', "''"),
                id.replace('\'', "''")
            ),
            Vec::new(),
        )
        .await
        .expect("seed system key");
}

#[tokio::test(flavor = "multi_thread")]
async fn reingest_drops_a_drawer_key_the_file_no_longer_declares() {
    let (engine, sink) = ingest_sink().await;

    let first = ingest(&sink, V1, None).await;
    let id = headline_id(&first);
    let props = stored_properties(&engine, &id).await;
    assert_eq!(
        props.get("leads-to").and_then(|v| v.as_str()),
        Some("m1"),
        "precondition: the first ingest stored the authored key; got {props:?}"
    );

    // Store-managed key, unrepresentable in the file.
    seed_system_key(&engine, &id, "_sys", "keep-me").await;

    ingest(&sink, V2, Some(&first)).await;

    let props = stored_properties(&engine, &id).await;
    assert!(
        !props.contains_key("leads-to"),
        "the file no longer declares `leads-to`, so it must not survive in the store; got {props:?}"
    );
    assert_eq!(
        props.get("contributes-to").and_then(|v| v.as_str()),
        Some("m1"),
        "the renamed key must be stored; got {props:?}"
    );
    assert_eq!(
        props.get("compass").and_then(|v| v.as_str()),
        Some("problem"),
        "an unchanged authored key must survive; got {props:?}"
    );
    assert_eq!(
        props.get("_sys").and_then(|v| v.as_str()),
        Some("keep-me"),
        "`_`-prefixed system keys are store-managed and never authored in a file — ingest must \
         preserve them; got {props:?}"
    );
}

/// The scope guard: a UI/agent write names only the field it changes, so the
/// merge must stay insert-only for it. Only ingest declares authority over the
/// whole property set.
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_user_write_still_merges_and_deletes_nothing() {
    let (engine, sink) = ingest_sink().await;
    let first = ingest(&sink, V1, None).await;
    let id = headline_id(&first);

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.clone()));
    params.insert("compass".into(), Value::String("goal".into()));
    sink.apply_ingest_batch(vec![("update".to_string(), params)])
        .await
        .expect("partial update");

    let props = stored_properties(&engine, &id).await;
    assert_eq!(
        props.get("compass").and_then(|v| v.as_str()),
        Some("goal"),
        "the written key must change; got {props:?}"
    );
    assert_eq!(
        props.get("leads-to").and_then(|v| v.as_str()),
        Some("m1"),
        "a partial write names no authority over peer keys — they must survive; got {props:?}"
    );
}
