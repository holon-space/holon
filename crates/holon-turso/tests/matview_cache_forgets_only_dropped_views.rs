//! Contract: a view drop invalidates exactly the dropped view and the views
//! that depend on it in the shared known-view cache.
//!
//! Wider than that and every drop anywhere costs every other view a
//! `sqlite_master` probe on its next `ensure_view`. Narrower than that and a
//! view chained on a dropped base stays "known" after it is gone, so
//! `ensure_view` skips its `CREATE` and the next read fails.

use std::collections::HashMap;
use std::sync::Arc;

use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn live_database() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')")
        .await
        .expect("create items");
    handle
        .execute(
            "INSERT INTO items (id, content) VALUES ('a', 'x')",
            Vec::new(),
        )
        .await
        .expect("insert item");
    handle
}

fn manager(handle: &DbHandle) -> MatviewManager {
    MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())))
}

/// `sqlite_master` probes the manager has issued: a cache hit issues none.
fn probes(manager: &MatviewManager) -> u64 {
    manager.cache_metrics().1
}

#[tokio::test]
async fn dropping_one_view_keeps_an_unrelated_view_known() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let dropped = manager
        .ensure_view("SELECT id FROM items")
        .await
        .expect("ensure dropped");
    let unrelated = manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("ensure unrelated");

    handle
        .execute_ddl(&format!("DROP VIEW IF EXISTS {dropped}"))
        .await
        .expect("drop");

    let before = probes(&manager);
    manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("re-ensure unrelated");
    assert_eq!(
        probes(&manager),
        before,
        "dropping {dropped} made the unrelated {unrelated} unknown: it paid a sqlite_master probe"
    );

    let creates_before = manager.cache_metrics().2;
    manager
        .ensure_view("SELECT id FROM items")
        .await
        .expect("re-ensure dropped");
    assert_eq!(
        manager.cache_metrics().2,
        creates_before + 1,
        "the dropped view {dropped} stayed known, so ensure_view did not recreate it"
    );
}

#[tokio::test]
async fn dropping_a_base_view_forgets_the_view_chained_on_it() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let base = manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("ensure base");
    let chained_sql = format!("SELECT id FROM {base}");
    let chained = manager
        .ensure_view(&chained_sql)
        .await
        .expect("ensure chained");

    // The same cascade the actor's reap and the base-view rebuild perform:
    // dependents first, then the base, each through the DDL boundary.
    handle
        .execute_ddl(&format!("DROP VIEW IF EXISTS {chained}"))
        .await
        .expect("drop chained");
    handle
        .execute_ddl(&format!("DROP VIEW IF EXISTS {base}"))
        .await
        .expect("drop base");

    manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("recreate base");
    let recreated = manager
        .ensure_view(&chained_sql)
        .await
        .expect("recreate chained");
    let rows = handle
        .query(&format!("SELECT id FROM {recreated}"), HashMap::new())
        .await
        .unwrap_or_else(|e| {
            panic!("the chained view {recreated} was not recreated after its base was dropped: {e}")
        });
    assert_eq!(rows.len(), 1, "the recreated chained view holds the item");
}

#[tokio::test]
async fn dropping_only_the_base_view_forgets_its_dependents() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let base = manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("ensure base");
    let chained_sql = format!("SELECT id FROM {base}");
    let chained = manager
        .ensure_view(&chained_sql)
        .await
        .expect("ensure chained");

    handle
        .execute_ddl(&format!("DROP VIEW IF EXISTS {base}"))
        .await
        .expect("drop base");

    let before = probes(&manager);
    manager
        .ensure_view(&chained_sql)
        .await
        .expect("re-ensure chained");
    assert_eq!(
        probes(&manager),
        before + 1,
        "dropping the base {base} left the chained {chained} known, so ensure_view trusted \
         the cache instead of checking sqlite_master"
    );
}
