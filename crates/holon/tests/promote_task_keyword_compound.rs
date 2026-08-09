//! `block.promote_task_keyword` — the engine-level compound that turns a
//! just-authored leading task keyword into `task_state` + stripped `content`,
//! driven by DIRECT engine calls (no editor involved).
//!
//! What these tests retire: the undo risk. A promotion is TWO field writes but
//! ONE user gesture, so a single Cmd-Z must restore both the text and the
//! absent task state. They also pin the refusal contract — a guard that
//! declines still commits the typed text verbatim, so a keystroke is never
//! lost.

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

async fn promote(
    engine: &BackendEngine,
    id: &str,
    typed: &str,
    keyword: &str,
) -> HashMap<String, Value> {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("typed".into(), Value::String(typed.to_string()));
    params.insert("keyword".into(), Value::String(keyword.to_string()));
    let outcome = engine
        .execute_operation(
            &EntityName::new("block"),
            "promote_task_keyword",
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("promote_task_keyword {id}: {e:#}"));
    match outcome.response {
        Some(Value::Object(map)) => map,
        other => panic!("promote_task_keyword must return a disclosure payload, got {other:?}"),
    }
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

fn payload_str(payload: &HashMap<String, Value>, key: &str) -> String {
    payload
        .get(key)
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("payload is missing {key:?}: {payload:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Fire
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn promotion_strips_the_keyword_and_sets_the_task_state() {
    let engine = block_engine().await;
    create_block(&engine, "block:p", "TODO").await;

    let payload = promote(&engine, "block:p", "TODO buy milk", "TODO").await;
    assert_eq!(payload_str(&payload, "outcome"), "promoted");
    assert_eq!(payload_str(&payload, "keyword"), "TODO");

    assert_eq!(
        col(&engine, "block:p", "content").await.as_deref(),
        Some("buy milk"),
        "the keyword must be stripped out of the content"
    );
    assert_eq!(
        prop(&engine, "block:p", "task_state").await.as_deref(),
        Some("TODO")
    );
    assert!(
        prop(&engine, "block:p", "task_state_category")
            .await
            .is_some(),
        "the category sidecar must be paired with the keyword"
    );
}

// ---------------------------------------------------------------------------
// One gesture = one undo press
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn promotion_is_one_composite_undo_entry() {
    let engine = block_engine().await;
    create_block(&engine, "block:u", "TODO").await;
    promote(&engine, "block:u", "TODO buy milk", "TODO").await;

    assert!(engine.can_undo().await, "the promotion must be undoable");

    // The composite's SHAPE, asserted before any behaviour: both constituents
    // are in the one entry. Without this the test passes for a compound that
    // never writes `task_state` at all — undo would have nothing to restore,
    // and every behavioural assertion below would still hold.
    let forward = undo_entry_fields(&engine, "ops").await;
    assert_eq!(
        forward.len(),
        2,
        "the promotion is exactly TWO constituents in ONE entry, got {forward:?}"
    );
    assert_eq!(
        forward.iter().collect::<BTreeSet<_>>(),
        ["content".to_string(), "task_state".to_string()]
            .iter()
            .collect::<BTreeSet<_>>(),
        "the entry must carry BOTH the content and the task_state write, got {forward:?}"
    );
    let inverse = undo_entry_fields(&engine, "inverse_ops").await;
    assert_eq!(
        inverse,
        vec!["task_state".to_string(), "content".to_string()],
        "inverses run leaf-first (reverse of the forward order), got {inverse:?}"
    );

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);

    assert_eq!(
        prop(&engine, "block:u", "task_state").await,
        None,
        "ONE undo press must clear the task_state the promotion added"
    );
    assert_eq!(
        col(&engine, "block:u", "content").await.as_deref(),
        Some("TODO"),
        "the SAME undo press must restore the pre-promotion text"
    );
    assert!(
        !engine.can_undo().await,
        "the promotion must be a SINGLE entry — a second undo would mean two"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn redo_re_promotes_identically() {
    let engine = block_engine().await;
    create_block(&engine, "block:r", "TODO").await;
    promote(&engine, "block:r", "TODO buy milk", "TODO").await;

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);

    assert_eq!(
        col(&engine, "block:r", "content").await.as_deref(),
        Some("buy milk"),
        "redo must re-apply the stripped content"
    );
    assert_eq!(
        prop(&engine, "block:r", "task_state").await.as_deref(),
        Some("TODO"),
        "redo must re-apply the task_state"
    );
}

// ---------------------------------------------------------------------------
// The marks-trap regression: a later content edit must NOT clear task_state
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn the_next_content_edit_preserves_the_task_state() {
    let engine = block_engine().await;
    create_block(&engine, "block:m", "TODO").await;
    promote(&engine, "block:m", "TODO buy milk", "TODO").await;

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

// ---------------------------------------------------------------------------
// Refuse — losslessly
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_proposal_commits_the_typed_text_verbatim() {
    let engine = block_engine().await;
    create_block(&engine, "block:x", "shop").await;

    // `MAYBE` is not in the engine's vocabulary, so the guard declines.
    let payload = promote(&engine, "block:x", "MAYBE buy milk", "MAYBE").await;
    assert_eq!(payload_str(&payload, "outcome"), "refused");
    assert_eq!(payload_str(&payload, "reason"), "not_keyword_headed");

    assert_eq!(
        col(&engine, "block:x", "content").await.as_deref(),
        Some("MAYBE buy milk"),
        "a refusal must never lose the keystroke — the typed text lands verbatim"
    );
    assert_eq!(
        prop(&engine, "block:x", "task_state").await,
        None,
        "a refused promotion must leave the block plain"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_already_tasked_block_is_refused_and_still_commits() {
    let engine = block_engine().await;
    create_block(&engine, "block:a", "TODO").await;
    promote(&engine, "block:a", "TODO buy milk", "TODO").await;

    let payload = promote(&engine, "block:a", "DONE buy milk", "DONE").await;
    assert_eq!(payload_str(&payload, "reason"), "already_tasked");
    assert_eq!(
        prop(&engine, "block:a", "task_state").await.as_deref(),
        Some("TODO"),
        "promotion is one-shot: the existing state must not be re-labelled"
    );
    assert_eq!(
        col(&engine, "block:a", "content").await.as_deref(),
        Some("DONE buy milk"),
        "the typed text still lands verbatim"
    );
}

/// The re-ingest-suppression guard, engine-side: a block whose text is ALREADY
/// keyword-headed carries no task state, so guard 1 cannot protect it — the
/// delta condition must.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_already_keyword_headed_is_refused() {
    let engine = block_engine().await;
    create_block(&engine, "block:k", "TODO list ideas").await;

    let payload = promote(&engine, "block:k", "TODO list ideasX", "TODO").await;
    assert_eq!(
        payload_str(&payload, "reason"),
        "prior_content_already_keyword_headed"
    );
    assert_eq!(prop(&engine, "block:k", "task_state").await, None);
    assert_eq!(
        col(&engine, "block:k", "content").await.as_deref(),
        Some("TODO list ideasX")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disagreeing_proposal_is_refused_rather_than_silently_corrected() {
    let engine = block_engine().await;
    create_block(&engine, "block:d", "TODO").await;

    // The guard resolves TODO from the text; the caller proposed DONE.
    let payload = promote(&engine, "block:d", "TODO buy milk", "DONE").await;
    assert_eq!(payload_str(&payload, "reason"), "proposal_disagrees");
    assert_eq!(prop(&engine, "block:d", "task_state").await, None);
    assert_eq!(
        col(&engine, "block:d", "content").await.as_deref(),
        Some("TODO buy milk")
    );
}
