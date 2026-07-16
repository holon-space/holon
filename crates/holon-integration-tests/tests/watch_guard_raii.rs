//! H1 regression (devlog 2026-07-06 fable review): watcher refcount RAII.
//!
//! Read-only snapshots (`ensure_watching` / `snapshot_reactive` — the MCP
//! `describe_ui`, PBT assertion, and TUI snapshot paths) must NOT pin a
//! block's watcher, and dropping the last `WatchGuard` (the ReactiveShell
//! lifecycle) must abort the watcher task and release its reactive state.
//! Before the fix, every read bumped the refcount with no matching release,
//! so `unwatch`'s refcount==0 branch was unreachable for any block ever
//! snapshotted: the tokio watcher + CDC stream leaked for the app's lifetime.
//!
//! @pbt kind harness
//! @pbt covers watcher-refcount-raii — H1 watcher refcount RAII on read-only snapshots

use std::sync::Arc;

use holon::api::repository::CoreOperations;
use holon::di::StorageSelector;
use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_integration_tests::TestEnvironment;

#[test]
fn read_snapshots_do_not_pin_watchers_and_guard_drop_releases() {
    // SUT owns its runtime; plain #[test] + block_on keeps the runtime Drop
    // on the main thread (same shape as loro_memory_start_app.rs).
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new_with_backend(runtime, StorageSelector::LoroMemory)
        .expect("new_with_backend(LoroMemory)");
    env.start_app(false).await.expect("start_app (LoroMemory)");

    let backend = env
        .loro_backend()
        .expect("LoroMemory start_app must register a loro_backend")
        .clone();
    let reactive = env
        .reactive_engine
        .get()
        .expect("start_app(LoroMemory) must resolve a ReactiveEngine")
        .clone();

    let root = backend
        .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
        .await
        .expect("create root");

    // ── Read path: N snapshots start the watcher once and never pin it ──
    assert_eq!(reactive.watcher_refcount(&root.id), None);
    for _ in 0..5 {
        let _ = reactive.snapshot_reactive(&root.id);
    }
    assert_eq!(
        reactive.watcher_refcount(&root.id),
        Some(0),
        "read-only snapshots must leave the refcount at 0"
    );
    let watchers_after_reads = reactive.active_watcher_count();
    for _ in 0..5 {
        let _ = reactive.ensure_watching(&root.id);
    }
    assert_eq!(
        reactive.active_watcher_count(),
        watchers_after_reads,
        "repeated reads must not grow the active-watch set"
    );
    assert_eq!(reactive.watcher_refcount(&root.id), Some(0));

    // ── Counting path: guards pin; the last drop aborts + removes ──
    let services: Arc<dyn BuilderServices> = reactive.clone();
    let (_rows_a, guard_a) = reactive.acquire_watch(&root.id, services.clone());
    assert_eq!(reactive.watcher_refcount(&root.id), Some(1));
    let (_rows_b, guard_b) = reactive.acquire_watch(&root.id, services.clone());
    assert_eq!(reactive.watcher_refcount(&root.id), Some(2));

    // Interleaved reads still don't count.
    for _ in 0..5 {
        let _ = reactive.snapshot_reactive(&root.id);
    }
    assert_eq!(reactive.watcher_refcount(&root.id), Some(2));

    drop(guard_a);
    assert_eq!(reactive.watcher_refcount(&root.id), Some(1));
    drop(guard_b);
    assert_eq!(
        reactive.watcher_refcount(&root.id),
        None,
        "last guard drop must abort the watcher and release reactive state"
    );

    // ── watch_live's LiveBlock carries the guard: dropping it releases ──
    let live = reactive.watch_live(&root.id, services.clone());
    assert!(
        live.watch_guard.is_some(),
        "engine watch_live must return a guard-bearing LiveBlock"
    );
    assert_eq!(reactive.watcher_refcount(&root.id), Some(1));
    drop(live);
    assert_eq!(reactive.watcher_refcount(&root.id), None);

    // ── A later read re-warms a fresh watcher, unpinned ──
    let _ = reactive.ensure_watching(&root.id);
    assert_eq!(reactive.watcher_refcount(&root.id), Some(0));
}
