//! `set_field("source_text")` — the SOURCE CHANNEL the editable surface commits
//! through, driven by DIRECT engine calls (no editor involved).
//!
//! The store's convergence is the parse: keyword-headed source spells the task
//! it names, source without a keyword clears the task state, and both columns
//! land as ONE reversible gesture. There is no promotion op and no refusal —
//! the parse is total, which is what retired the whole proposal/refusal
//! machinery and the DOUBLING shape it produced.
//!
//! Beside it, the #64 contract lock: `set_field("content")` writes one column
//! and NEVER re-derives the task state, whoever writes it.

use std::collections::BTreeSet;
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
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The same production SqlOnly block wiring `undo_inverse_wave1` uses: the CRUD
/// authority plus the structural provider.
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
async fn col(engine: &BackendEngine, id: &str, column: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT {column} FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("column query");
    rows.first()
        .and_then(|r| r.get(column))
        .and_then(|v| v.as_string())
        .map(str::to_string)
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

/// The `field` param of every op in one half of the newest journaled
/// `UndoEntry`, read out of the persisted `undo_log` snapshot
/// (`SqlUndoStore` serializes the whole `UndoStack`). This is the only way an
/// integration test can inspect the composite's CONSTITUENTS rather than infer
/// them from the state they happened to leave behind — a compound that never
/// writes `task_state` is invisible to a behavioural assertion, because the
/// state it forgot to set is also the state undo has nothing to restore.
async fn undo_entry_fields(engine: &BackendEngine, half: &str) -> Vec<String> {
    let rows = engine
        .db_handle()
        .query(
            "SELECT state_json FROM undo_log WHERE id = 0",
            HashMap::new(),
        )
        .await
        .expect("undo_log query");
    let json = rows
        .first()
        .and_then(|r| r.get("state_json"))
        .and_then(|v| v.as_string())
        .expect("the undo snapshot must be persisted")
        .to_string();
    let stack: serde_json::Value = serde_json::from_str(&json).expect("undo snapshot is JSON");
    let entry = stack["undo"]
        .as_array()
        .and_then(|entries| entries.last())
        .unwrap_or_else(|| panic!("the undo stack must hold an entry; snapshot={json}"));
    entry[half]
        .as_array()
        .unwrap_or_else(|| panic!("{half} must be an array; entry={entry}"))
        .iter()
        .map(|op| {
            op["params"]["field"]
                .as_str()
                .unwrap_or_else(|| panic!("every constituent is a set_field; op={op}"))
                .to_string()
        })
        .collect()
}

/// The `value` param of the op writing `field` in one half of the newest
/// journaled `UndoEntry`. Where [`undo_entry_fields`] proves WHICH constituents
/// the compound recorded, this proves what each one would WRITE — the only way
/// to separate an engine that decided to restore the wrong text from a store
/// that declined to keep the right text.
async fn undo_entry_values(engine: &BackendEngine, half: &str, field: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            "SELECT state_json FROM undo_log WHERE id = 0",
            HashMap::new(),
        )
        .await
        .expect("undo_log query");
    let json = rows
        .first()
        .and_then(|r| r.get("state_json"))
        .and_then(|v| v.as_string())
        .expect("the undo snapshot must be persisted")
        .to_string();
    let stack: serde_json::Value = serde_json::from_str(&json).expect("undo snapshot is JSON");
    // After an undo the entry has moved to the redo stack, so look in both.
    let entry = ["undo", "redo"]
        .iter()
        .filter_map(|s| stack[s].as_array())
        .flatten()
        .next_back()
        .unwrap_or_else(|| panic!("a journaled entry must exist; snapshot={json}"));
    entry[half]
        .as_array()
        .unwrap_or_else(|| panic!("{half} must be an array; entry={entry}"))
        .iter()
        .find(|op| op["params"]["field"].as_str() == Some(field))
        .map(|op| {
            op["params"]["value"]
                .as_str()
                .unwrap_or_else(|| panic!("the {field} constituent must carry a value; op={op}"))
                .to_string()
        })
}

/// The SPECIFIED empty-remainder case: source that is nothing but the keyword
/// is the empty-titled task `** TODO` spells on disk, and the store holds
/// exactly that.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_remainder_promotion_converges_to_an_empty_titled_task() {
    let engine = block_engine().await;
    create_block(&engine, "block:er", "").await;
    set_field_as_user(&engine, "block:er", "source_text", "TODO ").await;
    assert_eq!(
        col(&engine, "block:er", "content").await.as_deref(),
        Some(""),
        "the keyword was the whole text, so the promotion empties the content"
    );
    assert_eq!(
        prop(&engine, "block:er", "task_state").await.as_deref(),
        Some("TODO")
    );

    // A source write that only changes the TASK STATE is still one gesture:
    // the content constituent is a no-op here, and a delta-only vacuity test
    // would have made this press unundoable.
    assert!(
        engine.can_undo().await,
        "an empty-titled promotion must be undoable"
    );
    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::NoChange);
    assert_eq!(
        undo_entry_values(&engine, "inverse_ops", "content")
            .await
            .as_deref(),
        Some(""),
        "the source channel's inverse restores the CONVERGED value it replaced, \
         not the raw text — which is what lets the undo chain walk back cleanly"
    );
    assert_eq!(
        col(&engine, "block:er", "content").await.as_deref(),
        Some(""),
        "the block keeps the empty content it had before the write"
    );
    assert_eq!(
        prop(&engine, "block:er", "task_state").await,
        None,
        "undo restores the state the gesture replaced — the block was not a task, \
         so it is not one again. Under the promotion compound this could not \
         happen: its inverse restored the VERBATIM typed text, which the store \
         re-converged straight back into the same task."
    );
}

/// A bare keyword written as ORDINARY CONTENT converges the same way — the
/// empty-remainder case is a property of the bytes, not of the promotion op.
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_keyword_written_as_content_converges_to_an_empty_titled_task() {
    let engine = block_engine().await;
    create_block(&engine, "block:bare", "").await;
    set_field(&engine, "block:bare", "content", "TODO").await;

    assert_eq!(
        col(&engine, "block:bare", "content").await.as_deref(),
        Some("")
    );
    assert_eq!(
        prop(&engine, "block:bare", "task_state").await.as_deref(),
        Some("TODO"),
        "`** TODO` on disk is an empty-titled task, so the store holds one"
    );
}

/// A later CONTENT edit is one column: it must not clear the task state the
/// source write established, and it must not disturb the mark pipeline.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_content_edit_preserves_the_task_state() {
    let engine = block_engine().await;
    create_block(&engine, "block:m", "").await;
    set_field(&engine, "block:m", "source_text", "TODO buy milk").await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:m".to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert(
        "value".into(),
        Value::String("buy milk and [[Bread]]".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
        .expect("follow-up content edit");

    assert_eq!(
        prop(&engine, "block:m", "task_state").await.as_deref(),
        Some("TODO"),
        "task_state is NOT a function of content — a later edit must not clear it"
    );
    // The authored link is adopted by the dispatcher's mark follow-up: the
    // label survives in the content and the marks column is populated, so the
    // promotion did not disturb the mark pipeline.
    assert_eq!(
        col(&engine, "block:m", "content").await.as_deref(),
        Some("buy milk and Bread"),
        "the live-authored link must still be stripped to its label"
    );
    let marks = col(&engine, "block:m", "marks").await.unwrap_or_default();
    assert!(
        marks.contains("Link"),
        "the derived link mark must still be minted after a promotion; marks={marks:?}"
    );
}

/// The #64 boundary witness, deliberately unchanged: keyword-headed bytes
/// arriving on the CONTENT channel are stored, not parsed — the block keeps the
/// task state it had and the text lands verbatim.
#[tokio::test(flavor = "multi_thread")]
async fn the_content_channel_never_re_derives_the_task_state() {
    let engine = block_engine().await;
    create_block(&engine, "block:a", "TODO").await;
    set_field(&engine, "block:a", "source_text", "TODO buy milk").await;

    set_field(&engine, "block:a", "content", "DONE buy milk").await;
    assert_eq!(
        prop(&engine, "block:a", "task_state").await.as_deref(),
        Some("TODO"),
        "promotion is one-shot: the existing state must not be re-labelled"
    );
    assert_eq!(
        col(&engine, "block:a", "content").await.as_deref(),
        Some("DONE buy milk"),
        "the typed text still lands verbatim on the CONTENT channel"
    );
}

/// The #64 contract lock, stated on the plain channel with no compound in the
/// way: an agent writing keyword-shaped text into a task's content gets it
/// stored, not parsed. If the `source_text` work ever leaks into `content`,
/// this reds.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_content_write_on_a_tasked_block_is_not_re_parsed() {
    let engine = block_engine().await;
    create_block(&engine, "block:agentc", "").await;
    set_field(&engine, "block:agentc", "source_text", "TODO milk").await;

    set_field(&engine, "block:agentc", "content", "DONE later").await;

    assert_eq!(
        col(&engine, "block:agentc", "content").await.as_deref(),
        Some("DONE later"),
        "the content channel stores what it is given"
    );
    assert_eq!(
        prop(&engine, "block:agentc", "task_state").await.as_deref(),
        Some("TODO"),
        "and never re-derives the task state from it"
    );
}

// ---------------------------------------------------------------------------
// The SOURCE channel — the editable surface's commit
// ---------------------------------------------------------------------------

/// THE DOUBLING SHAPE, and the contract that retires it. The editable surface
/// seeds the source projection (`TODO milk`) and commits the buffer back
/// unchanged. On the content channel that would store `TODO milk` INSIDE a
/// block already carrying `TODO` — a keyword gained per focus cycle. On the
/// source channel the buffer is parsed, so re-committing an untouched buffer is
/// a no-op in meaning: the block is the same task it already was.
#[tokio::test(flavor = "multi_thread")]
async fn recommitting_the_source_projection_does_not_double_the_keyword() {
    let engine = block_engine().await;
    create_block(&engine, "block:src", "").await;
    set_field(&engine, "block:src", "source_text", "TODO milk").await;
    assert_eq!(
        col(&engine, "block:src", "content").await.as_deref(),
        Some("milk"),
        "precondition: the block is a task whose content is the bare title"
    );

    // What the editor holds on focus, committed back untouched.
    set_field(&engine, "block:src", "source_text", "TODO milk").await;

    assert_eq!(
        col(&engine, "block:src", "content").await.as_deref(),
        Some("milk"),
        "the keyword belongs to `task_state`; re-committing the source must not fold it into \
         the content"
    );
    assert_eq!(
        prop(&engine, "block:src", "task_state").await.as_deref(),
        Some("TODO"),
        "and the task survives its own source being rewritten"
    );
}

/// EXPLICIT DEMOTION. The user deletes the keyword out of the editable surface
/// and commits `milk`. There is no demote op and no gesture to recognise — the
/// parse simply finds no keyword, so `task_state` is cleared. This is the half
/// that `set_field("content")` structurally cannot do.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_keyword_from_the_source_demotes_the_block() {
    let engine = block_engine().await;
    create_block(&engine, "block:demote", "").await;
    set_field(&engine, "block:demote", "source_text", "TODO milk").await;
    assert_eq!(
        prop(&engine, "block:demote", "task_state").await.as_deref(),
        Some("TODO"),
        "precondition: it is a task"
    );

    set_field(&engine, "block:demote", "source_text", "milk").await;

    assert_eq!(
        prop(&engine, "block:demote", "task_state").await.as_deref(),
        Some(""),
        "the source carries no keyword, so the block is no longer a task — cleared with the \
         empty keyword, the same way `cycle_task_state` clears it"
    );
    assert_eq!(
        col(&engine, "block:demote", "content").await.as_deref(),
        Some("milk")
    );
}

/// The source channel PROMOTES as readily as it demotes — the parse is total
/// and symmetric, with no one-shot guard anywhere in it.
#[tokio::test(flavor = "multi_thread")]
async fn writing_a_keyword_into_the_source_promotes_a_plain_block() {
    let engine = block_engine().await;
    create_block(&engine, "block:srcp", "milk").await;

    set_field(&engine, "block:srcp", "source_text", "TODO milk").await;

    assert_eq!(
        col(&engine, "block:srcp", "content").await.as_deref(),
        Some("milk")
    );
    assert_eq!(
        prop(&engine, "block:srcp", "task_state").await.as_deref(),
        Some("TODO")
    );
}

/// The source channel reads the DOCUMENT's vocabulary, not the defaults — the
/// same authority every other task-keyword seam consults. In a document
/// declaring `NEXT WAITING | DONE`, `NEXT x` is a task and `TODO x` is prose.
#[tokio::test(flavor = "multi_thread")]
async fn the_source_channel_parses_under_the_documents_vocabulary() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:sv1").await;
    create_child(&engine, "block:sv2", "block:errands").await;

    set_field(&engine, "block:sv1", "source_text", "NEXT call bank").await;
    assert_eq!(
        prop(&engine, "block:sv1", "task_state").await.as_deref(),
        Some("NEXT")
    );
    assert_eq!(
        col(&engine, "block:sv1", "content").await.as_deref(),
        Some("call bank")
    );

    set_field(&engine, "block:sv2", "source_text", "TODO buy milk").await;
    assert_eq!(
        prop(&engine, "block:sv2", "task_state")
            .await
            .unwrap_or_default(),
        "",
        "TODO is prose in this document — parsing it as a keyword would write a state the \
         parser erases on the next re-ingest"
    );
    assert_eq!(
        col(&engine, "block:sv2", "content").await.as_deref(),
        Some("TODO buy milk")
    );
}

/// A source write is ONE undoable gesture across both fields — the #64 Inc 4b
/// contract, now carried by the channel the editor actually uses.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_write_is_one_composite_undo_entry() {
    let engine = block_engine().await;
    // Created EMPTY so the content constituent carries a real delta: a wholly
    // vacuous write is deliberately never journaled.
    create_block(&engine, "block:srcu", "").await;
    set_field_as_user(&engine, "block:srcu", "source_text", "TODO milk").await;
    assert_eq!(
        prop(&engine, "block:srcu", "task_state").await.as_deref(),
        Some("TODO")
    );

    let forward = undo_entry_fields(&engine, "ops").await;
    assert_eq!(
        forward.iter().collect::<BTreeSet<_>>(),
        ["content".to_string(), "task_state".to_string()]
            .iter()
            .collect::<BTreeSet<_>>(),
        "one source write journals BOTH constituents, got {forward:?}"
    );
    let inverse = undo_entry_fields(&engine, "inverse_ops").await;
    assert_eq!(
        inverse,
        vec!["task_state".to_string(), "content".to_string()],
        "inverses run leaf-first, got {inverse:?}"
    );

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        prop(&engine, "block:srcu", "task_state")
            .await
            .unwrap_or_default(),
        "",
        "ONE press takes the task state back off"
    );
    assert_eq!(
        col(&engine, "block:srcu", "content").await.as_deref(),
        Some(""),
        "and restores the text it was derived from — which is plain, so nothing re-converges"
    );
}

/// The RETIRED guard's state, asserted unreachable. A block whose text is
/// keyword-headed and whose `task_state` is absent used to be a legal state the
/// compound had to refuse to re-promote; the ruling makes it illegal, so the
/// `create` that used to produce it converges instead — and there is nothing
/// left for a re-promotion guard to protect.
#[tokio::test(flavor = "multi_thread")]
async fn creating_a_block_with_keyword_headed_content_converges_it() {
    let engine = block_engine().await;
    create_block(&engine, "block:k", "TODO list ideas").await;

    assert_eq!(
        col(&engine, "block:k", "content").await.as_deref(),
        Some("list ideas"),
        "a create cannot mint the illegal state either"
    );
    assert_eq!(
        prop(&engine, "block:k", "task_state").await.as_deref(),
        Some("TODO")
    );
}

/// Convergence is a property of the BYTES, not of the channel: an agent's plain
/// content write of keyword-headed text on an UNTASKED block converges too.
#[tokio::test(flavor = "multi_thread")]
async fn set_field_converges_however_the_text_arrives() {
    let engine = block_engine().await;
    create_block(&engine, "block:agent", "").await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:agent".into()));
    params.insert("field".into(), Value::String("content".into()));
    params.insert("value".into(), Value::String("TODO x".into()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::Sync,
        )
        .await
        .expect("set_field content");

    assert_eq!(
        col(&engine, "block:agent", "content").await.as_deref(),
        Some("x"),
        "the keyword is not content — it is the task state, on every write path"
    );
    assert_eq!(
        prop(&engine, "block:agent", "task_state").await.as_deref(),
        Some("TODO"),
        "an agent write converges exactly as a keystroke does"
    );
}

// ---------------------------------------------------------------------------
// The owning document's `#+TODO:` vocabulary is the authority
// ---------------------------------------------------------------------------

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

/// A USER-origin field write — the only origin that journals an undo entry.
async fn set_field_as_user(engine: &BackendEngine, id: &str, field: &str, value: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::User,
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

/// The persisted form of `#+TODO: NEXT WAITING | DONE` on a `Page`-tagged
/// document row, plus one child under it.
/// Serialized here rather than hand-written so the fixture cannot drift from
/// what `OrgDocumentExt::set_todo_keywords` actually persists.
fn declared_vocabulary() -> String {
    use holon_api::TaskState;
    serde_json::to_string(&[
        TaskState::active("NEXT"),
        TaskState::active("WAITING"),
        TaskState::done("DONE"),
    ])
    .expect("TaskState serializes")
}

async fn page_with_declared_vocabulary(engine: &BackendEngine, page: &str, child: &str) {
    create_block(engine, page, "Errands").await;
    set_field(engine, page, "todo_keywords", &declared_vocabulary()).await;
    tag_as_page(engine, page).await;
    create_child(engine, child, page).await;
}

/// The document's declared DONE list decides the category sidecar, not a
/// hardcoded one.
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_done_list_decides_the_category() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:cat").await;

    set_field(&engine, "block:cat", "source_text", "WAITING on reply").await;
    assert_eq!(
        prop(&engine, "block:cat", "task_state_category")
            .await
            .as_deref(),
        Some("active"),
        "WAITING is declared active, not done"
    );
}

/// The convergence rule is PER DOCUMENT and it runs on the generic write path,
/// not only inside the promotion compound: in a document declaring
/// `#+TODO: NEXT WAITING | DONE`, a plain `set_field` of `NEXT ...` converges
/// and one of `TODO ...` does not. A future format provider that declares no
/// keywords converges nothing by the same rule.
#[tokio::test(flavor = "multi_thread")]
async fn set_field_converges_by_the_documents_own_vocabulary() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:v1").await;
    create_child(&engine, "block:v2", "block:errands").await;

    set_field(&engine, "block:v1", "content", "NEXT call bank").await;
    assert_eq!(
        col(&engine, "block:v1", "content").await.as_deref(),
        Some("call bank")
    );
    assert_eq!(
        prop(&engine, "block:v1", "task_state").await.as_deref(),
        Some("NEXT"),
        "NEXT is declared by this document, so those bytes are a task"
    );

    set_field(&engine, "block:v2", "content", "TODO buy milk").await;
    assert_eq!(
        col(&engine, "block:v2", "content").await.as_deref(),
        Some("TODO buy milk"),
        "TODO is ordinary prose here — converging it would write a state the parser erases"
    );
    assert_eq!(prop(&engine, "block:v2", "task_state").await, None);
}

/// THE ADJACENT CELL of the vocabulary hole, locked at the store. A PLAIN block
/// whose text merely has the SHAPE of a keyword the document does not declare
/// must not gain a blank `task_state`: the parse names no keyword and there is
/// nothing to clear, so the task-state constituent is skipped entirely rather
/// than writing `""` onto a block that never was a task.
///
/// Without the skip, every source commit of `ASAP …` / `API …` / `PR …` in a
/// `#+TODO:`-declaring document would leave a blank task state behind — a state
/// no other write path produces, and one the org round trip has no way to
/// spell.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_write_that_declares_nothing_leaves_a_plain_block_plain() {
    let engine = block_engine().await;
    page_with_declared_vocabulary(&engine, "block:errands", "block:plainkw").await;

    set_field(&engine, "block:plainkw", "source_text", "ASAP call Bob").await;

    assert_eq!(
        col(&engine, "block:plainkw", "content").await.as_deref(),
        Some("ASAP call Bob"),
        "an undeclared token is ordinary prose here, so the text lands whole"
    );
    assert_eq!(
        prop(&engine, "block:plainkw", "task_state").await,
        None,
        "the block was never a task and names no keyword, so NOTHING is written — \
         not the empty keyword either"
    );
    assert_eq!(
        prop(&engine, "block:plainkw", "task_state_category").await,
        None,
        "and no category sidecar is left behind"
    );
}

/// A document that declares NOTHING keeps the defaults — the precedence rule,
/// pinned so the vocabulary read cannot regress into an empty vocabulary.
#[tokio::test(flavor = "multi_thread")]
async fn an_undeclaring_document_keeps_the_defaults() {
    let engine = block_engine().await;
    create_block(&engine, "block:inbox", "Inbox").await;
    tag_as_page(&engine, "block:inbox").await;
    create_child(&engine, "block:plain", "block:inbox").await;

    set_field(&engine, "block:plain", "source_text", "TODO buy milk").await;
    assert_eq!(
        prop(&engine, "block:plain", "task_state").await.as_deref(),
        Some("TODO")
    );
}
