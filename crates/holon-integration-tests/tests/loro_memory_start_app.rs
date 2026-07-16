//! a2 (ADR 0004 Phase 9, part (a)): backend selection at `start_app`, done
//! through DI.
//!
//! `TestEnvironment::new_with_backend(StorageSelector::LoroMemory)` →
//! `start_app` must assemble a no-Turso container (no `BackendEngine`),
//! register the Loro storage adapter + the block-query frontend, and
//! **resolve** a `FrontendSession` + `ReactiveEngine` that render structural
//! blocks straight from the Loro tree. The Turso machinery (MCP, CDC watches,
//! org sync, seed priming) is simply not registered in this wiring.
//!
//! This is also the headless V2-gate extension for the slice: it proves the
//! whole no-Turso render path (snapshot → `UiEvent` → `ReactiveEngine`) paints
//! real rows and tracks live churn (add + delete) — everything except the GPUI
//! window, which needs a display.
//!
//! @pbt kind harness
//! @pbt covers backend-selection-di — a2 backend selection at start_app via DI

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::api::repository::CoreOperations;
use holon::di::StorageSelector;
use holon_api::BlockContent;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::reactive::ReactiveRenderedRows;
use holon_integration_tests::TestEnvironment;

#[test]
fn loro_memory_start_app_paints_and_tracks_churn() {
    // The SUT owns its own runtime (mirrors the phased runner). A plain `#[test]`
    // + `block_on` keeps the runtime's Drop on the main thread (dropping a
    // Runtime inside async context panics).
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run_test(runtime.clone()));
}

async fn run_test(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new_with_backend(runtime, StorageSelector::LoroMemory)
        .expect("new_with_backend(LoroMemory)");
    assert_eq!(env.storage(), StorageSelector::LoroMemory);

    // start_app takes the no-Turso DI branch: builds the container, resolves a
    // session with no engine + a reactive engine over `block_query`.
    env.start_app(false).await.expect("start_app (LoroMemory)");
    assert!(env.is_running(), "no-Turso session should be running");
    // `block_query` is now total (present in both wirings); confirm the no-Turso
    // session's source produces a snapshot rather than asserting mere presence.
    env.session()
        .block_query()
        .snapshot()
        .await
        .expect("start_app(LoroMemory) block_query source must snapshot");

    // The storage adapter the SUT seeds / mutates (the no-Turso wiring has no
    // engine dispatch).
    let backend = env
        .loro_backend()
        .expect("LoroMemory start_app must register a loro_backend")
        .clone();
    let reactive = env
        .reactive_engine
        .get()
        .expect("start_app(LoroMemory) must resolve a ReactiveEngine")
        .clone();

    // Seed a small tree.
    let root = backend
        .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
        .await
        .expect("create root");
    let mut children = Vec::new();
    for label in ["alpha", "beta"] {
        let child = backend
            .create_block(root.id.clone(), BlockContent::text(label), None)
            .await
            .expect("create child");
        children.push(child.id);
    }

    // Paint: the reactive engine must converge to the seeded children.
    let results = reactive.ensure_watching(&root.id);
    let seeded: Vec<String> = children.iter().map(|c| c.to_string()).collect();
    poll_rows_until(&results, "initial", |ids| {
        seeded.iter().all(|c| ids.contains(c))
    })
    .await;

    // Churn 1: add a child — it must appear (re-emit, not a stale one-shot).
    let added = backend
        .create_block(root.id.clone(), BlockContent::text("added"), None)
        .await
        .expect("create added");
    let added_id = added.id.to_string();
    poll_rows_until(&results, "after-add", |ids| ids.contains(&added_id)).await;

    // Churn 2: delete a child — it must disappear.
    let removed_id = children[0].to_string();
    backend
        .delete_block(children[0].as_str())
        .await
        .expect("delete child");
    poll_rows_until(&results, "after-delete", |ids| !ids.contains(&removed_id)).await;
}

/// Stage 4 (ADR 0004 Phase 9): a no-Turso `LoroMemory` session exposes the
/// **operation** capability over Loro-native providers — `operation_engine()`
/// is `Some`, block operations are advertised, and a mutation dispatched
/// through the session lands in the same Loro doc the read path observes. This
/// proves the cache-free `LoroBlockOperations` → `OperationDispatcher` →
/// `DispatchingOperationEngine` wiring works end-to-end without Turso.
#[test]
fn loro_memory_operation_engine_mutates_shared_doc() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime
        .clone()
        .block_on(run_operation_engine_test(runtime.clone()));
}

async fn run_operation_engine_test(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new_with_backend(runtime, StorageSelector::LoroMemory)
        .expect("new_with_backend(LoroMemory)");
    env.start_app(false).await.expect("start_app (LoroMemory)");

    // The no-Turso session now carries the operation capability (Stage 4).
    let session = env.session();
    assert!(
        session.operation_engine().is_some(),
        "LoroMemory session must expose an operation engine"
    );
    assert!(
        session.has_operation("block", "set_field").await,
        "block set_field must be advertised by the Loro operation engine"
    );

    // Seed a block straight on the backend (the read side).
    let backend = env
        .loro_backend()
        .expect("LoroMemory start_app must register a loro_backend")
        .clone();
    let root = backend
        .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
        .await
        .expect("create root");
    let child = backend
        .create_block(root.id.clone(), BlockContent::text("before"), None)
        .await
        .expect("create child");

    // Mutate through the session's operation path (not the backend directly).
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(child.id.to_string()));
    params.insert("field".to_string(), Value::String("content".to_string()));
    params.insert("value".to_string(), Value::String("after".to_string()));
    session
        .execute_operation(&EntityName::from("block"), "set_field", params)
        .await
        .expect("set_field via no-Turso operation engine");

    // The write must be visible in the shared Loro doc the reads observe.
    let updated = backend
        .get_block(child.id.as_str())
        .await
        .expect("get_block after mutation");
    assert_eq!(
        updated.content, "after",
        "operation-engine mutation must land in the shared Loro doc"
    );

    // `set_field("content")` is reversible: undo restores the prior text, redo
    // re-applies it — full round-trip through the no-Turso undo stack.
    assert!(session.can_undo().await, "set_field must be undoable");
    assert!(
        session.undo().await.expect("undo").applied(),
        "undo applied"
    );
    assert_eq!(
        backend
            .get_block(child.id.as_str())
            .await
            .expect("get_block after undo")
            .content,
        "before",
        "undo must restore the prior content"
    );
    assert!(
        session.can_redo().await,
        "redo must be available after undo"
    );
    assert!(
        session.redo().await.expect("redo").applied(),
        "redo applied"
    );
    assert_eq!(
        backend
            .get_block(child.id.as_str())
            .await
            .expect("get_block after redo")
            .content,
        "after",
        "redo must re-apply the mutation"
    );
}

fn row_ids(results: &ReactiveRenderedRows) -> Vec<String> {
    let (_expr, rows) = results.snapshot();
    rows.iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

async fn poll_rows_until(
    results: &ReactiveRenderedRows,
    label: &str,
    want: impl Fn(&[String]) -> bool,
) {
    for _ in 0..200u32 {
        if want(&row_ids(results)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "[{label}] reactive rows never converged; got {:?}",
        row_ids(results)
    );
}
