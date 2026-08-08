//! Undo fidelity under the editor's real keystroke shape: each keystroke is a
//! FIRE-AND-FORGET `set_field(content)` task (`holon_frontend::operations::
//! dispatch_operation` spawns and never awaits), so N writes to ONE block are
//! in flight at once. Without per-entity serialization the provider's
//! "read prior content" and its UPDATE straddle another keystroke's write, so
//! the stored inverse skips characters and the entries land out of write order
//! — every later undo then legitimately fails its precondition and is DROPPED.
//! Dogfood 2026-08-08 (BugFunnel F3): six `undo: dropped stale entry` ERRORs
//! for one typed word.

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

const TYPED: &str = "second line";
const BLOCK_ID: &str = "block:typed";

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    descriptors,
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

async fn seed_empty_block(engine: &BackendEngine) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(BLOCK_ID.to_string()));
    params.insert("content".into(), Value::String(String::new()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .expect("seed block");
}

async fn content_of(engine: &BackendEngine) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{BLOCK_ID}'"),
            HashMap::new(),
        )
        .await
        .expect("content query");
    rows.first()
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

fn keystroke_params(prefix: String, seq: usize) -> StorageEntity {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(BLOCK_ID.to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String(prefix));
    params.insert("write_seq".into(), Value::Integer(seq as i64));
    params
}

/// Type `TYPED` one character at a time the way the editor does: one spawned,
/// un-awaited task per keystroke, issued in order with a small gap. Returns
/// once every write has completed.
async fn type_word_as_the_editor_does(engine: &Arc<BackendEngine>) {
    let mut handles = Vec::new();
    for (i, prefix) in prefixes().into_iter().enumerate() {
        let engine = Arc::clone(engine);
        handles.push(tokio::spawn(async move {
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "set_field",
                    keystroke_params(prefix, i + 1),
                    OpOrigin::User,
                )
                .await
                .expect("keystroke write");
        }));
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    for h in handles {
        h.await.expect("keystroke task");
    }
}

/// The same typing with every write awaited before the next is issued — the
/// single-writer reference the concurrent run must match.
async fn type_word_one_write_at_a_time(engine: &BackendEngine) {
    for (i, prefix) in prefixes().into_iter().enumerate() {
        engine
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                keystroke_params(prefix, i + 1),
                OpOrigin::User,
            )
            .await
            .expect("keystroke write");
    }
}

fn prefixes() -> Vec<String> {
    (1..=TYPED.chars().count())
        .map(|n| TYPED.chars().take(n).collect())
        .collect()
}

/// Press undo until the stack is empty, returning the content after each press.
/// Fails loud on the first dropped entry — a dropped entry IS the bug.
async fn undo_to_empty(engine: &BackendEngine) -> Vec<Option<String>> {
    let mut walk = Vec::new();
    for press in 0..40 {
        match engine.undo().await.expect("undo call") {
            UndoOutcome::Empty => return walk,
            UndoOutcome::Applied => walk.push(content_of(engine).await),
            other => panic!(
                "undo press {press} lost a step: {other:?} (walk so far: {walk:?}) — every undo \
                 entry recorded by our own typing must still verify"
            ),
        }
    }
    panic!("undo did not reach an empty stack in 40 presses (walk: {walk:?})");
}

/// The whole property, stated differentially: typing a word with the editor's
/// real concurrency must leave exactly the history that typing it one awaited
/// write at a time leaves — same number of undo presses, same state after each
/// one, no entry dropped. Comparing against the single-writer run (rather than
/// against a hand-written expectation) keeps the property about concurrency and
/// nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_keystrokes_keep_every_undo_step() {
    let sequential = block_engine().await;
    seed_empty_block(&sequential).await;
    type_word_one_write_at_a_time(&sequential).await;
    assert_eq!(content_of(&sequential).await.as_deref(), Some(TYPED));
    let reference_walk = undo_to_empty(&sequential).await;
    assert!(
        reference_walk.len() >= 2,
        "reference walk must have real steps to compare against, got {reference_walk:?}"
    );

    let concurrent = block_engine().await;
    seed_empty_block(&concurrent).await;
    type_word_as_the_editor_does(&concurrent).await;
    assert_eq!(
        content_of(&concurrent).await.as_deref(),
        Some(TYPED),
        "the last keystroke's text must be what the block holds"
    );

    assert_eq!(
        undo_to_empty(&concurrent).await,
        reference_walk,
        "concurrent typing must leave the same undo history as sequential typing"
    );
}

/// The inverse-fidelity half, stated on the entries themselves: every stored
/// inverse must restore the state its own forward op replaced. A stale prior
/// read shows up here as a skipped character even before any precondition is
/// checked.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_keystroke_inverses_skip_no_character() {
    let engine = block_engine().await;
    seed_empty_block(&engine).await;

    type_word_as_the_editor_does(&engine).await;

    let mut seen = Vec::new();
    loop {
        match engine.undo().await.expect("undo call") {
            UndoOutcome::Empty => break,
            UndoOutcome::Applied => seen.push(content_of(&engine).await.unwrap_or_default()),
            other => panic!("undo dropped a step: {other:?}"),
        }
    }
    // Every restored state must be a prefix the user actually typed through
    // (or the empty pre-typing state) — never a state that was skipped over.
    let typed_states: Vec<String> = std::iter::once(String::new()).chain(prefixes()).collect();
    for state in &seen {
        assert!(
            typed_states.contains(state),
            "undo restored {state:?}, which was never a state the user typed \
             (states: {typed_states:?})"
        );
    }
    assert_eq!(
        seen.last().map(String::as_str),
        Some(""),
        "the last undo must land on the pre-typing state, got {seen:?}"
    );
}
