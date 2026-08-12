//! Contract: the view-existence state `MatviewManager`s share is scoped to ONE
//! database.
//!
//! Two properties that pull in opposite directions, so both are asserted here:
//!
//! - SHARED within a database: two managers racing to watch the same query
//!   materialise its view once between them, not once each.
//! - SEPARATE across databases: view names are content hashes of their SELECT
//!   (`compute_view_name`), so two databases needing the same query produce the
//!   SAME name for DIFFERENT views. State spanning them would report the second
//!   database's view as already present and skip its `CREATE`, leaving its
//!   queries reading a view that does not exist.
//!
//! The first property only has teeth CONCURRENTLY. Sequentially the
//! `sqlite_master` probe in `ensure_view` already stops the second create, so a
//! sequential version passes even with per-manager state and proves nothing.

use std::collections::HashMap;
use std::sync::Arc;

use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const ITEMS_DDL: &str = "CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')";
const WATCHED_SELECT: &str = "SELECT id, content FROM items";

/// A live in-memory database. The backend is leaked so its actor outlives the
/// call — the handle is inert once the actor stops.
async fn live_database() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle.execute_ddl(ITEMS_DDL).await.expect("create items");
    handle
}

fn manager(handle: &DbHandle) -> MatviewManager {
    MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())))
}

/// Whether `view` is a real object in `handle`'s schema — read from
/// `sqlite_master`, never from a manager's cache, which is what is under test.
async fn view_exists_in_schema(handle: &DbHandle, view: &str) -> bool {
    let escaped = view.replace('\'', "''");
    !handle
        .query(
            &format!("SELECT name FROM sqlite_master WHERE name = '{escaped}'"),
            HashMap::new(),
        )
        .await
        .expect("sqlite_master read")
        .is_empty()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_managers_on_one_database_materialise_the_view_once() {
    let handle = live_database().await;
    let first = manager(&handle);
    let second = manager(&handle);

    let (a, b) = tokio::join!(
        first.ensure_view(WATCHED_SELECT),
        second.ensure_view(WATCHED_SELECT)
    );
    assert_eq!(
        a.expect("first ensure"),
        b.expect("second ensure"),
        "both managers name the same view"
    );

    let creates = first.cache_metrics().2 + second.cache_metrics().2;
    assert_eq!(
        creates, 1,
        "the racing managers issued {creates} CREATE MATERIALIZED VIEW statements for one view; \
         each pays full Turso IVM warm-up, so a second is duplicated work on every interaction \
         that opens a watch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_database_materialises_its_own_view() {
    let one = live_database().await;
    let two = live_database().await;

    let view_one = manager(&one)
        .ensure_view(WATCHED_SELECT)
        .await
        .expect("ensure in first database");
    let view_two = manager(&two)
        .ensure_view(WATCHED_SELECT)
        .await
        .expect("ensure in second database");

    assert_eq!(
        view_one, view_two,
        "the same SELECT hashes to the same name in both databases — the premise that makes \
         cross-database sharing dangerous"
    );
    assert!(
        view_exists_in_schema(&one, &view_one).await,
        "{view_one} missing from the first database's schema"
    );
    assert!(
        view_exists_in_schema(&two, &view_two).await,
        "{view_two} missing from the SECOND database's schema — its CREATE was skipped because \
         another database had already made that name known"
    );
}

/// A reaped view must be rebuilt by the next manager that needs it.
///
/// `reap_view` (`turso.rs`) drops a `watch_view_*` when its last lease goes,
/// and it is the actor — not any `MatviewManager` — that decides to. A manager
/// whose cache still lists the reaped name hands back a view that is no longer
/// in the schema, and every query against it reads a view that does not exist.
/// The cache must therefore stop being trusted once the actor reaps anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reaped_view_is_rebuilt_by_the_next_manager() {
    let handle = live_database().await;
    let view = manager(&handle)
        .ensure_view(WATCHED_SELECT)
        .await
        .expect("first ensure");

    // Take and release the only lease, which is what makes the actor reap.
    let grant = handle
        .acquire_view_lease(&view, WATCHED_SELECT)
        .await
        .expect("lease");
    handle.release_view_lease(&view, grant);
    // The release is fire-and-forget; wait for the drop to land in the schema.
    for _ in 0..200 {
        if !view_exists_in_schema(&handle, &view).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !view_exists_in_schema(&handle, &view).await,
        "{view} was not reaped, so this test never reaches what it exists to check"
    );

    let rebuilt = manager(&handle)
        .ensure_view(WATCHED_SELECT)
        .await
        .expect("ensure after reap");

    assert_eq!(rebuilt, view, "same SELECT, same view name");
    assert!(
        view_exists_in_schema(&handle, &rebuilt).await,
        "{rebuilt} was handed back as usable but is NOT in the schema — the cache \
         still lists a view the actor reaped, so every query against it reads a view \
         that does not exist"
    );
}

/// A view dropped as a REBUILD DEPENDENT must be rebuilt by the next manager.
///
/// `reconcile_named_view` cascade-drops everything selecting from a base view
/// whose SELECT changed (`drop_dependent_views`), and it runs at runtime —
/// `holon-advice`'s synthesis and the MCP sidecar both reconcile named views
/// after watch views already exist. Nothing about that drop passes through the
/// actor's reap path, so it is a second way the cache can outlive its views.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_dropped_as_a_rebuild_dependent_is_rebuilt_by_the_next_manager() {
    const BASE: &str = "lat_floor_base";
    let watch_select = format!("SELECT id, content FROM {BASE}");
    let handle = live_database().await;

    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        BASE,
        "SELECT id, content FROM items",
    )
    .await
    .expect("create base view");

    let view = manager(&handle)
        .ensure_view(&watch_select)
        .await
        .expect("watch over the base view");
    assert!(
        view_exists_in_schema(&handle, &view).await,
        "{view} not created"
    );

    // Change the base's SELECT: its dependents are cascade-dropped.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        BASE,
        "SELECT id, content, 1 AS extra FROM items",
    )
    .await
    .expect("rebuild base view");
    assert!(
        !view_exists_in_schema(&handle, &view).await,
        "{view} survived the base rebuild, so this test never reaches what it checks"
    );

    let rebuilt = manager(&handle)
        .ensure_view(&watch_select)
        .await
        .expect("ensure after the cascade drop");

    assert_eq!(rebuilt, view, "same SELECT, same view name");
    assert!(
        view_exists_in_schema(&handle, &rebuilt).await,
        "{rebuilt} was handed back as usable but is NOT in the schema — it was dropped as \
         a rebuild dependent and the cache never learned"
    );
}
