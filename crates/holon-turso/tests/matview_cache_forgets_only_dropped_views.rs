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

/// Wait for the next CDC batch on `stream`, failing if none arrives.
async fn next_batch(stream: &mut holon_turso::turso::RowChangeStream, what: &str) {
    use futures::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap_or_else(|_| panic!("{what}: no CDC batch within 5s — the stream went silent"))
        .unwrap_or_else(|| panic!("{what}: the stream ended"));
}

async fn insert_item(handle: &DbHandle, id: &str) {
    handle
        .execute(
            &format!("INSERT INTO items (id, content) VALUES ('{id}', 'y')"),
            Vec::new(),
        )
        .await
        .expect("insert item");
}

#[tokio::test]
async fn dropping_every_watch_view_recreates_the_ones_a_subscriber_listens_to() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let (listened, mut stream) = manager
        .ensure_and_subscribe("SELECT id FROM items", None)
        .await
        .expect("subscribe");
    let unlistened = manager
        .ensure_view("SELECT id, content FROM items")
        .await
        .expect("ensure unlistened");

    // Another manager on the same database drops them, as `full_sync`'s does.
    let dropper = self::manager(&handle);
    dropper.drop_stale_views().await.expect("drop stale views");

    let views: Vec<String> = handle
        .query(
            "SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%'",
            HashMap::new(),
        )
        .await
        .expect("list views")
        .into_iter()
        .filter_map(|row| match row.get("name") {
            Some(holon_api::Value::String(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        views,
        vec![listened.clone()],
        "only {listened}, which has a subscriber, is recreated; {unlistened} stays dropped"
    );
    insert_item(&handle, "b").await;
    next_batch(&mut stream, "the subscriber of the recreated view").await;
}

#[tokio::test]
async fn a_view_dropped_before_its_subscriber_registers_is_recreated_for_it() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let sql = "SELECT id FROM items";
    let view = manager.ensure_view(sql).await.expect("ensure");
    handle
        .execute_ddl(&format!("DROP VIEW IF EXISTS {view}"))
        .await
        .expect("drop");

    let mut stream = manager
        .subscribe_ensured(&view, sql, None)
        .await
        .expect("subscribe");
    insert_item(&handle, "b").await;
    next_batch(
        &mut stream,
        "a subscriber whose view was dropped before it registered",
    )
    .await;
}
