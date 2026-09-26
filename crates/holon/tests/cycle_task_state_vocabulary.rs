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

/// A page whose `#+TODO:` ring is `ring`, with one child `child`.
async fn page_with_ring(
    engine: &BackendEngine,
    page: &str,
    child: &str,
    ring: &[holon_api::TaskState],
) {
    create_block(engine, page, "Shipping").await;
    set_field(
        engine,
        page,
        "todo_keywords",
        &serde_json::to_string(ring).expect("TaskState serializes"),
    )
    .await;
    tag_as_page(engine, page).await;
    create_child(engine, child, page).await;
}

async fn set_state_as(engine: &BackendEngine, id: &str, keyword: &str, origin: OpOrigin) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String("task_state".to_string()));
    params.insert("value".into(), Value::String(keyword.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "set_field", params, origin)
        .await
        .unwrap_or_else(|e| panic!("set_field task_state on {id}: {e:#}"));
}

/// The category of a written keyword is the one the block's OWN document ring
/// gives it: `SHIPPED` is done in `TODO | SHIPPED`, `CANCELLED` is active in
/// `TODO CANCELLED | DONE`, whatever org's default lists say.
#[tokio::test(flavor = "multi_thread")]
async fn a_written_keyword_takes_its_documents_category() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    page_with_ring(
        &engine,
        "block:shipping",
        "block:s1",
        &[TaskState::active("TODO"), TaskState::done("SHIPPED")],
    )
    .await;
    page_with_ring(
        &engine,
        "block:triage",
        "block:t1",
        &[
            TaskState::active("TODO"),
            TaskState::active("CANCELLED"),
            TaskState::done("DONE"),
        ],
    )
    .await;

    set_state_as(&engine, "block:s1", "SHIPPED", OpOrigin::User).await;
    assert_eq!(
        prop(&engine, "block:s1", "task_state_category")
            .await
            .as_deref(),
        Some("done"),
        "SHIPPED is done in `TODO | SHIPPED`"
    );
    set_state_as(&engine, "block:t1", "CANCELLED", OpOrigin::User).await;
    assert_eq!(
        prop(&engine, "block:t1", "task_state_category")
            .await
            .as_deref(),
        Some("active"),
        "CANCELLED is active in `TODO CANCELLED | DONE`"
    );

    // Undo replays the prior keyword, classified by the same ring.
    set_state_as(&engine, "block:s1", "TODO", OpOrigin::User).await;
    engine.undo().await.expect("undo dispatch");
    engine.undo().await.expect("undo dispatch");
    assert_eq!(
        prop(&engine, "block:t1", "task_state").await.as_deref(),
        None,
        "the CANCELLED write is taken back"
    );
    assert_eq!(
        (
            prop(&engine, "block:s1", "task_state").await,
            prop(&engine, "block:s1", "task_state_category").await
        ),
        (Some("SHIPPED".to_string()), Some("done".to_string())),
        "undoing TODO restores SHIPPED as done"
    );
}

/// A created row that carries a keyword carries its category too, from its
/// document's ring.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_task_carries_its_documents_category() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    page_with_ring(
        &engine,
        "block:shipping",
        "block:s1",
        &[TaskState::active("TODO"), TaskState::done("SHIPPED")],
    )
    .await;
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:s2".to_string()));
    params.insert("content".into(), Value::String("Fresh row".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:shipping".to_string()),
    );
    params.insert("task_state".into(), Value::String("SHIPPED".to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
        .await
        .unwrap_or_else(|e| panic!("create a task: {e:#}"));
    assert_eq!(
        (
            prop(&engine, "block:s2", "task_state").await,
            prop(&engine, "block:s2", "task_state_category").await
        ),
        (Some("SHIPPED".to_string()), Some("done".to_string()))
    );
}

async fn move_block(engine: &BackendEngine, id: &str, parent: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("parent_id".into(), Value::String(parent.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("move {id} under {parent}: {e:#}"));
}

/// Two documents whose rings classify `CANCELLED` differently.
async fn two_rings(engine: &BackendEngine) {
    use holon_api::TaskState;

    page_with_ring(
        engine,
        "block:closing",
        "block:x",
        &[TaskState::active("TODO"), TaskState::done("CANCELLED")],
    )
    .await;
    page_with_ring(
        engine,
        "block:triage",
        "block:y",
        &[
            TaskState::active("TODO"),
            TaskState::active("CANCELLED"),
            TaskState::done("DONE"),
        ],
    )
    .await;
    set_state_as(engine, "block:x", "CANCELLED", OpOrigin::User).await;
}

async fn pair(engine: &BackendEngine, id: &str) -> (Option<String>, Option<String>) {
    (
        prop(engine, id, "task_state").await,
        prop(engine, id, "task_state_category").await,
    )
}

/// A task moved into a document whose ring classifies its keyword otherwise
/// takes that document's category, and undoing the move gives the old one back.
#[tokio::test(flavor = "multi_thread")]
async fn a_moved_task_takes_its_new_documents_category() {
    let engine = block_engine().await;
    two_rings(&engine).await;
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("CANCELLED".into()), Some("done".into()))
    );

    move_block(&engine, "block:x", "block:triage").await;
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("CANCELLED".into()), Some("active".into())),
        "CANCELLED is active in the document it moved to"
    );

    engine.undo().await.expect("undo dispatch");
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("CANCELLED".into()), Some("done".into())),
        "undoing the move puts it back under the ring that makes it done"
    );
}

/// A ring edit re-derives the category of the document's tasks.
#[tokio::test(flavor = "multi_thread")]
async fn a_ring_edit_rederives_its_documents_categories() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    two_rings(&engine).await;
    set_field(
        &engine,
        "block:closing",
        "todo_keywords",
        &serde_json::to_string(&[
            TaskState::active("TODO"),
            TaskState::active("CANCELLED"),
            TaskState::done("DONE"),
        ])
        .expect("TaskState serializes"),
    )
    .await;
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("CANCELLED".into()), Some("active".into())),
        "the ring now declares CANCELLED active"
    );
}

/// A keyword the document's ring does not declare is refused by name, and
/// nothing is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_keyword_its_documents_ring_does_not_declare_is_refused() {
    let engine = block_engine().await;
    page_with_ring(
        &engine,
        "block:shipping",
        "block:s1",
        &[
            holon_api::TaskState::active("NEXT"),
            holon_api::TaskState::done("SHIPPED"),
        ],
    )
    .await;
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:s1".to_string()));
    params.insert("field".into(), Value::String("task_state".to_string()));
    params.insert("value".into(), Value::String("TODO".to_string()));
    let err = engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
        .expect_err("TODO is not a keyword of `NEXT | SHIPPED`");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("TODO") && msg.contains("block:shipping") && msg.contains("NEXT | SHIPPED"),
        "the refusal names the keyword, the document and its ring: {msg}"
    );
    assert_eq!(pair(&engine, "block:s1").await, (None, None));
}

async fn try_op(engine: &BackendEngine, op: &str, params: &[(&str, &str)]) -> anyhow::Result<()> {
    let params: StorageEntity = params
        .iter()
        .map(|(k, v)| (Arc::from(*k), Value::String(v.to_string())))
        .collect();
    engine
        .execute_operation(&EntityName::new("block"), op, params, OpOrigin::User)
        .await
        .map(|_| ())
}

async fn parent_of(engine: &BackendEngine, id: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT parent_id AS v FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("parent query");
    rows.first()
        .and_then(|r| r.get("v"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// Page `block:closing` (`TODO | CANCELLED`) holding `block:x` = `TODO`, and
/// page `block:shipping` (`NEXT | SHIPPED`), which does not declare `TODO`.
async fn a_ring_without_todo(engine: &BackendEngine) {
    use holon_api::TaskState;

    page_with_ring(
        engine,
        "block:closing",
        "block:x",
        &[TaskState::active("TODO"), TaskState::done("CANCELLED")],
    )
    .await;
    page_with_ring(
        engine,
        "block:shipping",
        "block:s1",
        &[TaskState::active("NEXT"), TaskState::done("SHIPPED")],
    )
    .await;
    set_state_as(engine, "block:x", "TODO", OpOrigin::User).await;
}

fn assert_names_the_ring(result: anyhow::Result<()>, block: &str) {
    let msg = format!("{:#}", result.expect_err("the move must be refused"));
    assert!(
        msg.contains("\"TODO\"")
            && msg.contains(block)
            && msg.contains("block:shipping")
            && msg.contains("NEXT | SHIPPED"),
        "the refusal names the keyword, the block, the target document and its ring: {msg}"
    );
}

/// A task moved into a document whose ring does not declare its keyword is
/// refused before any write: org would write the headline without it.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_into_a_ring_that_lacks_the_keyword_is_refused() {
    let engine = block_engine().await;
    a_ring_without_todo(&engine).await;

    let moved = try_op(
        &engine,
        "move_block",
        &[("id", "block:x"), ("parent_id", "block:shipping")],
    )
    .await;
    assert_names_the_ring(moved, "block:x");
    assert_eq!(
        parent_of(&engine, "block:x").await.as_deref(),
        Some("block:closing")
    );
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("TODO".into()), Some("active".into()))
    );

    let reparented = try_op(
        &engine,
        "set_field",
        &[
            ("id", "block:x"),
            ("field", "parent_id"),
            ("value", "block:shipping"),
        ],
    )
    .await;
    assert_names_the_ring(reparented, "block:x");
    assert_eq!(
        parent_of(&engine, "block:x").await.as_deref(),
        Some("block:closing")
    );
}

/// The refusal covers every block the move hands to the target document, not
/// only the moved one.
#[tokio::test(flavor = "multi_thread")]
async fn a_subtree_move_is_refused_by_its_descendants_keyword() {
    let engine = block_engine().await;
    a_ring_without_todo(&engine).await;
    create_child(&engine, "block:holder", "block:closing").await;
    move_block(&engine, "block:x", "block:holder").await;

    let moved = try_op(
        &engine,
        "move_block",
        &[("id", "block:holder"), ("parent_id", "block:shipping")],
    )
    .await;
    assert_names_the_ring(moved, "block:x");
    assert_eq!(
        parent_of(&engine, "block:holder").await.as_deref(),
        Some("block:closing")
    );
}

/// Indenting under a sibling that is a page hands the block to that page's
/// document.
#[tokio::test(flavor = "multi_thread")]
async fn an_indent_into_a_ring_that_lacks_the_keyword_is_refused() {
    let engine = block_engine().await;
    a_ring_without_todo(&engine).await;
    move_block(&engine, "block:shipping", "block:closing").await;
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:x".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:closing".to_string()),
    );
    params.insert(
        "after_block_id".into(),
        Value::String("block:shipping".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::User,
        )
        .await
        .expect("place block:x right after the nested page");

    let indented = try_op(&engine, "indent", &[("id", "block:x")]).await;
    assert_names_the_ring(indented, "block:x");
    assert_eq!(
        parent_of(&engine, "block:x").await.as_deref(),
        Some("block:closing")
    );
}

/// A ring edit that drops a keyword one of the document's tasks carries is
/// refused: the file would lose that task's state.
#[tokio::test(flavor = "multi_thread")]
async fn a_ring_edit_that_drops_a_used_keyword_is_refused() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    a_ring_without_todo(&engine).await;
    let ring = serde_json::to_string(&[TaskState::active("NEXT"), TaskState::done("SHIPPED")])
        .expect("TaskState serializes");
    let edited = try_op(
        &engine,
        "set_field",
        &[
            ("id", "block:closing"),
            ("field", "todo_keywords"),
            ("value", &ring),
        ],
    )
    .await;
    let msg = format!("{:#}", edited.expect_err("the ring edit must be refused"));
    assert!(
        msg.contains("\"TODO\"") && msg.contains("block:x") && msg.contains("NEXT | SHIPPED"),
        "the refusal names the keyword, the block and the new ring: {msg}"
    );
    assert_eq!(
        pair(&engine, "block:x").await,
        (Some("TODO".into()), Some("active".into()))
    );
}

/// Page `block:closing` (`TODO WAITING |`) holding `holder`, whose child
/// `block:kid` is `WAITING`, and page `block:home`, which declares no ring.
async fn a_waiting_task_under(engine: &BackendEngine, holder: &str, content: &str) {
    use holon_api::TaskState;

    page_with_ring(
        engine,
        "block:closing",
        holder,
        &[TaskState::active("TODO"), TaskState::active("WAITING")],
    )
    .await;
    set_field(engine, holder, "content", content).await;
    create_child(engine, "block:kid", holder).await;
    set_state_as(engine, "block:kid", "WAITING", OpOrigin::Sync).await;
    create_block(engine, "block:home", "Home").await;
    tag_as_page(engine, "block:home").await;
}

async fn ids_where(engine: &BackendEngine, condition: &str) -> Vec<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT id FROM {BLOCK_WRITE_TABLE} WHERE {condition} ORDER BY id"),
            HashMap::new(),
        )
        .await
        .expect("id query");
    rows.iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("id column")
                .to_string()
        })
        .collect()
}

fn assert_names_waiting(result: anyhow::Result<()>, op: &str) {
    let msg = format!("{:#}", result.expect_err("the compound must be refused"));
    assert!(
        msg.starts_with(op) && msg.contains("\"WAITING\"") && msg.contains("block:kid"),
        "the refusal names the compound, the keyword and the block: {msg}"
    );
}

/// "Turn into page" mints a page that declares its source document's ring, so
/// every keyword of the subtree it takes stays a keyword of its document.
#[tokio::test(flavor = "multi_thread")]
async fn a_converted_page_declares_its_source_documents_ring() {
    let engine = block_engine().await;
    a_waiting_task_under(&engine, "block:origin", "Origin").await;
    let is_page = "id IN (SELECT block_id FROM block_tags WHERE tag = 'Page')";
    let pages_before = ids_where(&engine, is_page).await;

    try_op(
        &engine,
        "convert_block_to_page",
        &[("target", "block:origin"), ("destination_path", "")],
    )
    .await
    .expect("the convert applies");
    let page = parent_of(&engine, "block:kid")
        .await
        .expect("block:kid has a parent");
    assert!(
        !pages_before.contains(&page) && ids_where(&engine, is_page).await.contains(&page),
        "block:kid moved to the minted page, got {page}"
    );
    assert_eq!(
        prop(&engine, &page, "todo_keywords").await,
        prop(&engine, "block:closing", "todo_keywords").await
    );
    assert_eq!(
        pair(&engine, "block:kid").await,
        (Some("WAITING".into()), Some("active".into()))
    );

    engine.undo().await.expect("undo dispatch");
    assert_eq!(ids_where(&engine, is_page).await, pages_before);
    assert_eq!(
        parent_of(&engine, "block:kid").await.as_deref(),
        Some("block:origin")
    );
}

/// Merging a duplicate into a canonical in another document hands the
/// duplicate's children to that document; a keyword its ring lacks refuses
/// the merge before the canonical's body or children change.
#[tokio::test(flavor = "multi_thread")]
async fn a_merge_whose_moved_keyword_the_canonical_document_lacks_writes_nothing() {
    let engine = block_engine().await;
    a_waiting_task_under(&engine, "block:dup", "Dup body").await;
    create_child(&engine, "block:canon", "block:home").await;
    set_field(&engine, "block:canon", "content", "Canon").await;
    let blocks_before = ids_where(&engine, "1 = 1").await;

    let merged = try_op(
        &engine,
        "merge_blocks",
        &[("canonical", "block:canon"), ("duplicate", "block:dup")],
    )
    .await;
    assert_names_waiting(merged, "merge_blocks");
    assert_eq!(ids_where(&engine, "1 = 1").await, blocks_before);
    assert_eq!(
        parent_of(&engine, "block:kid").await.as_deref(),
        Some("block:dup")
    );
    assert!(
        !engine.can_undo().await,
        "a refused gesture journals nothing"
    );
}

/// A merge whose canonical adopts the duplicate's ring moves the duplicate's
/// children before the ring arrives, so it is refused, naming why.
#[tokio::test(flavor = "multi_thread")]
async fn a_merge_that_adopts_the_ring_its_moved_keyword_needs_is_refused_by_name() {
    let engine = block_engine().await;
    a_waiting_task_under(&engine, "block:holder", "Holder").await;
    let blocks_before = ids_where(&engine, "1 = 1").await;

    let merged = try_op(
        &engine,
        "merge_blocks",
        &[("canonical", "block:home"), ("duplicate", "block:closing")],
    )
    .await;
    let msg = format!("{:#}", merged.expect_err("the merge must be refused"));
    assert!(
        msg.starts_with("merge_blocks")
            && msg.contains("\"WAITING\"")
            && msg.contains("block:kid")
            && msg.contains("block:home")
            && msg.contains("adopts"),
        "the refusal names the keyword, the block, the document and why the adopted ring does \
         not admit it: {msg}"
    );
    assert_eq!(ids_where(&engine, "1 = 1").await, blocks_before);
}

/// Template `block:tpl` (in no document) whose child `block:tpl-kid` carries
/// `keyword`, instantiated under page `block:shipping`, whose ring is `ring`.
async fn instantiate_keyword_into_ring(
    engine: &BackendEngine,
    ring: &[holon_api::TaskState],
    keyword: &str,
) -> anyhow::Result<()> {
    page_with_ring(engine, "block:shipping", "block:s1", ring).await;
    create_block(engine, "block:tpl", "Template").await;
    set_field(engine, "block:tpl", "template", "weekly").await;
    create_child(engine, "block:tpl-kid", "block:tpl").await;
    set_field(engine, "block:tpl-kid", "content", "Review").await;
    set_state_as(engine, "block:tpl-kid", keyword, OpOrigin::Sync).await;
    try_op(
        engine,
        "instantiate_template",
        &[
            ("template_id", "block:tpl"),
            ("target_parent", "block:shipping"),
            ("context_key", "week-1"),
        ],
    )
    .await
}

/// A template keyword the target document's ring lacks refuses the whole
/// instantiation by name before any block is created.
#[tokio::test(flavor = "multi_thread")]
async fn a_template_keyword_the_target_ring_lacks_is_refused_before_any_create() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    let refused = instantiate_keyword_into_ring(
        &engine,
        &[TaskState::active("NEXT"), TaskState::done("SHIPPED")],
        "TODO",
    )
    .await;
    let msg = format!(
        "{:#}",
        refused.expect_err("the instantiation must be refused")
    );
    assert!(
        msg.starts_with("instantiate_template")
            && msg.contains("\"TODO\"")
            && msg.contains("block:tpl-kid")
            && msg.contains("block:shipping")
            && msg.contains("NEXT | SHIPPED"),
        "the refusal names the keyword, the template block and the target document: {msg}"
    );
    assert_eq!(
        ids_where(&engine, "parent_id = 'block:shipping'").await,
        vec!["block:s1".to_string()],
        "no instance block is created"
    );
}

/// An instance task takes its category from the target document's ring:
/// `CANCELLED` is done in the template's and active in the target's.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_task_takes_the_target_documents_category() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    instantiate_keyword_into_ring(
        &engine,
        &[
            TaskState::active("TODO"),
            TaskState::active("CANCELLED"),
            TaskState::done("DONE"),
        ],
        "CANCELLED",
    )
    .await
    .expect("the instantiation applies");
    let instance_kids = ids_where(
        &engine,
        "parent_id IN (SELECT id FROM block_raw WHERE parent_id = 'block:shipping' AND id != \
         'block:s1')",
    )
    .await;
    let [kid] = instance_kids.as_slice() else {
        panic!("one instance child, got {instance_kids:?}");
    };
    assert_eq!(
        pair(&engine, kid).await,
        (Some("CANCELLED".into()), Some("active".into()))
    );
}
