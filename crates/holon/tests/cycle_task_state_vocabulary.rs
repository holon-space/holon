//! `block.cycle_task_state` — the ring Cmd+Enter walks is the OWNING
//! DOCUMENT's `#+TODO:` vocabulary, not a hardcoded list.
//!
//! What these tests retire: silent data loss. A ring that writes `TODO` into a
//! document declaring `#+TODO: NEXT WAITING | DONE` stores a keyword the org
//! parser cannot read back — on the next cold-boot re-ingest the headline
//! `** TODO delta` returns as PLAIN BODY TEXT and the task is gone. The
//! invariant is therefore stated as MEMBERSHIP (every written keyword is a
//! keyword of this document), not as a list of expected strings.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The production SqlOnly block wiring: the CRUD authority plus the structural
/// provider. Test-local by intent — this suite must stay independent of the
/// promotion suite it mirrors.
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
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
            .with_operation_provider_factory(|backend| {
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
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block provider")
}

async fn create_block(engine: &BackendEngine, id: &str, content: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn create_child(engine: &BackendEngine, id: &str, parent: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(String::new()));
    params.insert("parent_id".into(), Value::String(parent.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn set_field(engine: &BackendEngine, id: &str, field: &str, value: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::Sync,
        )
        .await
        .unwrap_or_else(|e| panic!("set_field {field} on {id}: {e:#}"));
}

async fn tag_as_page(engine: &BackendEngine, id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("tag".into(), Value::String("Page".to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "add_tag", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("tag {id} as Page: {e:#}"));
}

async fn cycle(engine: &BackendEngine, id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "cycle_task_state",
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("cycle_task_state {id}: {e:#}"));
}

async fn prop(engine: &BackendEngine, id: &str, key: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT json_extract(properties, '$.{key}') AS v FROM {BLOCK_WRITE_TABLE} WHERE \
                 id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("prop query");
    rows.first()
        .and_then(|r| r.get("v"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// The persisted form of `#+TODO: NEXT WAITING | DONE` on a `Page`-tagged
/// document row. Serialized here rather than hand-written so the fixture cannot
/// drift from what `OrgDocumentExt::set_todo_keywords` actually persists.
fn declared_vocabulary() -> String {
    use holon_api::TaskState;
    serde_json::to_string(&[
        TaskState::active("NEXT"),
        TaskState::active("WAITING"),
        TaskState::done("DONE"),
    ])
    .expect("TaskState serializes")
}

/// UNIT-LEVEL STAND-IN for a `#+TODO:` header. This suite boots an engine with
/// no filesystem, so no org file can declare anything — the `set_field` below
/// impersonates the ingest. The production path (a real `FileSyncController`
/// ingest actually delivering the declaration) is proven by
/// `holon-integration-tests/tests/task_vocabulary_reaches_the_store.rs`; a
/// green here alone would not.
async fn page_with_declared_vocabulary(engine: &BackendEngine, page: &str, child: &str) {
    create_block(engine, page, "Errands").await;
    set_field(engine, page, "todo_keywords", &declared_vocabulary()).await;
    tag_as_page(engine, page).await;
    create_child(engine, child, page).await;
}

/// Every keyword the fixture document declares. A written state must be one of
/// these or the empty (not-a-task) state — anything else is unreadable to the
/// parser and vanishes on re-ingest.
const DECLARED: [&str; 3] = ["NEXT", "WAITING", "DONE"];

fn assert_declared(state: Option<&str>, step: &str) {
    let Some(state) = state else {
        panic!("{step}: the cycle wrote no task_state at all");
    };
    assert!(
        DECLARED.contains(&state),
        "{step}: cycle_task_state wrote {state:?}, which is NOT a keyword of this document's \
         vocabulary {DECLARED:?}. The org parser cannot read it back, so the next full re-ingest \
         turns the headline into plain body text and the task is silently lost."
    );
}

/// F3, the data-mutation bug: in a document declaring its own vocabulary the
/// ring must be that vocabulary. The membership assertion is the invariant; the
/// per-step equalities pin the ORDER (active before done, empty state first).
#[tokio::test(flavor = "multi_thread")]
async fn the_ring_is_the_documents_declared_vocabulary() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:c1").await;

    cycle(&engine, "block:c1").await;
    let first = prop(&engine, "block:c1", "task_state").await;
    assert_declared(first.as_deref(), "first cycle");
    assert_eq!(
        first.as_deref(),
        Some("NEXT"),
        "the first declared active keyword is the first ring stop"
    );

    cycle(&engine, "block:c1").await;
    let second = prop(&engine, "block:c1", "task_state").await;
    assert_declared(second.as_deref(), "second cycle");
    assert_eq!(second.as_deref(), Some("WAITING"));

    cycle(&engine, "block:c1").await;
    let third = prop(&engine, "block:c1", "task_state").await;
    assert_declared(third.as_deref(), "third cycle");
    assert_eq!(third.as_deref(), Some("DONE"));
}

/// The ring closes back onto the not-a-task state — a user must be able to undo
/// a task by cycling, in a custom vocabulary just as in the default one.
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_ring_closes_on_the_empty_state() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:c2").await;

    for _ in 0..4 {
        cycle(&engine, "block:c2").await;
    }
    assert_eq!(
        prop(&engine, "block:c2", "task_state").await.as_deref(),
        Some(""),
        "four cycles over a three-keyword vocabulary return to the empty state"
    );
}

/// The declared DONE list decides the category, so a ring that got only the
/// keywords right would still write a wrong `task_state_category` — and every
/// `task_state_category = 'active'` query would miss the block.
#[tokio::test(flavor = "multi_thread")]
async fn the_category_sidecar_follows_the_declared_done_list() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:c3").await;

    cycle(&engine, "block:c3").await;
    cycle(&engine, "block:c3").await;
    assert_eq!(
        prop(&engine, "block:c3", "task_state").await.as_deref(),
        Some("WAITING")
    );
    assert_eq!(
        prop(&engine, "block:c3", "task_state_category")
            .await
            .as_deref(),
        Some("active"),
        "WAITING is declared active, not done"
    );

    cycle(&engine, "block:c3").await;
    assert_eq!(
        prop(&engine, "block:c3", "task_state_category")
            .await
            .as_deref(),
        Some("done"),
        "DONE is declared done"
    );
}

/// REGRESSION LOCK. A document that declares nothing keeps the native ring
/// `"" -> TODO -> DOING -> DONE`. The DEFAULT vocabulary is an INGEST
/// tolerance set (it also admits LATER/NOW/CANCELLED/CLOSED so foreign vaults
/// parse); walking a user through those was never the behaviour, so this test
/// reds if the ring is naively built from the default keyword lists.
#[tokio::test(flavor = "multi_thread")]
async fn an_undeclaring_document_keeps_the_native_ring() {
    let engine = block_engine().await;
    create_block(&engine, "block:inbox", "Inbox").await;
    tag_as_page(&engine, "block:inbox").await;
    create_child(&engine, "block:plain", "block:inbox").await;

    let mut seen = Vec::new();
    for _ in 0..4 {
        cycle(&engine, "block:plain").await;
        seen.push(
            prop(&engine, "block:plain", "task_state")
                .await
                .unwrap_or_default(),
        );
    }
    assert_eq!(
        seen,
        vec![
            "TODO".to_string(),
            "DOING".to_string(),
            "DONE".to_string(),
            String::new()
        ],
        "the default ring must be exactly the native one"
    );
}

/// The LogSeq dialect rule (ForeignVaultCompat §4) survives the vocabulary
/// rewrite: an imported `LATER` block stays in the LogSeq ring rather than
/// snapping into the native one.
#[tokio::test(flavor = "multi_thread")]
async fn an_imported_logseq_keyword_stays_in_the_logseq_ring() {
    let engine = block_engine().await;
    create_block(&engine, "block:inbox", "Inbox").await;
    tag_as_page(&engine, "block:inbox").await;
    create_child(&engine, "block:later", "block:inbox").await;
    set_field(&engine, "block:later", "task_state", "LATER").await;

    cycle(&engine, "block:later").await;
    assert_eq!(
        prop(&engine, "block:later", "task_state").await.as_deref(),
        Some("NOW")
    );
    cycle(&engine, "block:later").await;
    assert_eq!(
        prop(&engine, "block:later", "task_state").await.as_deref(),
        Some("DONE")
    );
}

/// One gesture, one undo. The cycle is a compound only in that the engine
/// resolves the ring before writing; a single Cmd-Z must put the keyword back.
#[tokio::test(flavor = "multi_thread")]
async fn one_cycle_is_one_undoable_gesture() {
    use holon_api::UndoOutcome;

    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:c4").await;

    cycle(&engine, "block:c4").await;
    cycle(&engine, "block:c4").await;
    assert_eq!(
        prop(&engine, "block:c4", "task_state").await.as_deref(),
        Some("WAITING")
    );

    assert_eq!(
        engine.undo().await.expect("undo dispatch"),
        UndoOutcome::Applied,
        "the cycle must have journaled an undo entry"
    );
    assert_eq!(
        prop(&engine, "block:c4", "task_state").await.as_deref(),
        Some("NEXT"),
        "one undo steps back exactly one cycle"
    );
}
