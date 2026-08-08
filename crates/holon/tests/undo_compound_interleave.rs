//! The compound half of the keystroke race (BugFunnel task #29 round 2).
//!
//! `merge_blocks` and `convert_block_to_page` read the block they are about to
//! rewrite (their planners), write it through constituent dispatches, and
//! journal ONE composite entry — the same read-modify-write-journal step a
//! plain `set_field` performs, but assembled in the engine. The editor's
//! keystroke writes are spawned un-awaited
//! (`holon_frontend::operations::dispatch_operation`), so a keystroke can land
//! between a planner's read and its constituent write, or between that write
//! and the composite push. Either way the composite entry is born stale and the
//! first undo drops it.
//!
//! Both tests state one property: a compound issued WHILE the user is still
//! typing into the block it rewrites leaves a history that undoes cleanly all
//! the way back — no entry dropped, and the block ends where it started.

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
use holon_api::PAGE_TAG;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::link_parser::PageId;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const TYPED: &str = "typed while merging";

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    descriptors,
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

/// Fixture blocks are `Sync`-origin so they never enter the undo stack the
/// assertions inspect.
async fn create(engine: &BackendEngine, id: &str, parent_id: &str, content: &str, is_page: bool) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    if is_page {
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
        );
    }
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn content_of(engine: &BackendEngine, id: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("content query");
    rows.first()
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

async fn marks_of(engine: &BackendEngine, id: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT marks FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("marks query");
    rows.first()
        .and_then(|r| r.get("marks"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// A bold span over the first `1 + round` characters — the shape link
/// resolution writes, varied per round so consecutive writes are never vacuous.
fn marks_json(round: usize) -> String {
    format!("[{{\"kind\":\"Bold\",\"start\":0,\"end\":{}}}]", round + 1)
}

fn prefixes(text: &str) -> Vec<String> {
    (1..=text.chars().count())
        .map(|n| text.chars().take(n).collect())
        .collect()
}

/// Type into `id` the way the editor does — one spawned, un-awaited task per
/// keystroke, issued in order with a small gap — returning the handles so the
/// caller can fire a compound INTO the middle of the run.
fn type_into(engine: &Arc<BackendEngine>, id: &str) -> Vec<tokio::task::JoinHandle<()>> {
    prefixes(TYPED)
        .into_iter()
        .enumerate()
        .map(|(i, prefix)| {
            let engine = Arc::clone(engine);
            let id = id.to_string();
            tokio::spawn(async move {
                // Stagger inside the task so the caller can interleave a
                // compound after the first few keystrokes are already in flight.
                tokio::time::sleep(std::time::Duration::from_millis(i as u64)).await;
                let mut params: StorageEntity = HashMap::new();
                params.insert("id".into(), Value::String(id));
                params.insert("field".into(), Value::String("content".to_string()));
                params.insert("value".into(), Value::String(prefix));
                params.insert("write_seq".into(), Value::Integer(i as i64 + 1));
                engine
                    .execute_operation(
                        &EntityName::new("block"),
                        "set_field",
                        params,
                        OpOrigin::User,
                    )
                    .await
                    .expect("keystroke write");
            })
        })
        .collect()
}

/// Press undo until the stack is empty, failing loud on the first dropped
/// entry — a dropped entry IS the bug.
async fn undo_to_empty(engine: &BackendEngine) -> usize {
    for press in 0..60 {
        match engine.undo().await.expect("undo call") {
            UndoOutcome::Empty => return press,
            UndoOutcome::Applied | UndoOutcome::NoChange => {}
            other => panic!(
                "undo press {press} lost a step: {other:?} — every entry recorded by our own \
                 actions must still verify"
            ),
        }
    }
    panic!("undo did not reach an empty stack in 60 presses");
}

/// A merge fired while the canonical block is still being typed into: the
/// planner's content read, the constituent write and the composite journal push
/// must all fall on one side of every keystroke.
#[tokio::test(flavor = "multi_thread")]
async fn merge_while_typing_into_the_canonical_keeps_every_undo_step() {
    let engine = block_engine().await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", true).await;
    create(&engine, "block:canonical", &home, "", false).await;
    create(&engine, "block:duplicate", &home, "duplicate body", false).await;

    // Both in flight at once — the merge is a chord the user reaches for while
    // still typing, not a quiesced operation.
    let typing = type_into(&engine, "block:canonical");
    let merging = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            let mut params: StorageEntity = HashMap::new();
            params.insert(
                "canonical".into(),
                Value::String("block:canonical".to_string()),
            );
            params.insert(
                "duplicate".into(),
                Value::String("block:duplicate".to_string()),
            );
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "merge_blocks",
                    params,
                    OpOrigin::User,
                )
                .await
                .expect("merge_blocks dispatch");
        })
    };
    merging.await.expect("merge task");
    for h in typing {
        h.await.expect("keystroke task");
    }

    let presses = undo_to_empty(&engine).await;
    assert!(presses > 0, "the actions must have left undo entries");
    assert_eq!(
        content_of(&engine, "block:canonical").await.as_deref(),
        Some(""),
        "undoing everything must return the canonical block to its pre-typing state"
    );
    assert_eq!(
        content_of(&engine, "block:duplicate").await.as_deref(),
        Some("duplicate body"),
        "undoing the merge must restore the absorbed block"
    );
}

/// The same property for the block→page compound. Its constituents rewrite the
/// origin's MARKS (the content stays put and becomes the link label), so the
/// concurrent writer here is the un-awaited marks write link resolution
/// dispatches on a block whose text names a page. Typing is deliberately NOT
/// mixed in: a content write schedules a DERIVED marks write of its own AFTER
/// the op returns, which is a second, unrelated ordering hole (see the lane
/// report) — driving it here would prove something other than this window.
#[tokio::test(flavor = "multi_thread")]
async fn convert_while_marks_are_being_written_keeps_every_undo_step() {
    let engine = block_engine().await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", true).await;
    create(&engine, "block:origin", &home, "seed", false).await;
    create(&engine, "block:child", "block:origin", "child one", false).await;

    // The compound goes first and the writers pile up behind it — the shape a
    // chord fired mid-session has: the block is already under a compound when
    // the next write for it is dispatched.
    let converting = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            let mut params: StorageEntity = HashMap::new();
            params.insert("target".into(), Value::String("block:origin".to_string()));
            params.insert("destination_path".into(), Value::String("Home".to_string()));
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "convert_block_to_page",
                    params,
                    OpOrigin::User,
                )
                .await
                .expect("convert_block_to_page dispatch");
        })
    };
    // The writer that matters is the one that arrives in the window between the
    // compound's own marks write and its journal push. Waiting for the link
    // marks to appear puts it there by construction instead of by stopwatch:
    // under the hold it then queues until the compound is done, without it the
    // compound's entry is already stale when it is pushed.
    let racer = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            for spin in 0.. {
                assert!(spin < 100_000, "the convert never wrote its link marks");
                if marks_of(&engine, "block:origin")
                    .await
                    .is_some_and(|m| m.contains("Link"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let mut params: StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String("block:origin".to_string()));
            params.insert("field".into(), Value::String("marks".to_string()));
            params.insert("value".into(), Value::String(marks_json(7)));
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "set_field",
                    params,
                    OpOrigin::User,
                )
                .await
                .expect("racing marks write");
        })
    };
    converting.await.expect("convert task");
    racer.await.expect("racing marks task");

    let presses = undo_to_empty(&engine).await;
    assert!(presses > 0, "the actions must have left undo entries");
    assert_eq!(
        content_of(&engine, "block:origin").await.as_deref(),
        Some("seed"),
        "undoing everything must return the origin block to its pre-action state"
    );
    assert_eq!(
        marks_of(&engine, "block:origin").await,
        None,
        "undoing everything must return the origin's marks to their pre-action state"
    );
    assert_eq!(
        content_of(&engine, "block:child").await.as_deref(),
        Some("child one"),
        "the child must survive the convert/undo round trip"
    );
}
