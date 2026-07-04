//! Contract of the actor-owned matview lifecycle, driven through `DbHandle`.
//!
//! Every assertion here relies on the actor's FIFO command queue rather than on
//! sleeping: `release_view_lease` is fire-and-forget, but the very next
//! `db.query(..)` is queued behind it, so by the time that query answers the
//! release (and any reap it triggered) has already run to completion. A test
//! that needed a sleep would be evidence that the lifecycle is NOT confined to
//! command processing.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const PROBE_SELECT: &str = "SELECT id, content FROM lease_probe";
const PROBE_VIEW: &str = "watch_view_probe";

async fn booted() -> DbHandle {
    let (_backend, db) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso backend");
    db.execute_ddl("CREATE TABLE lease_probe (id TEXT PRIMARY KEY, content TEXT)")
        .await
        .expect("create probe table");
    // Leaked on purpose: the backend owns the connection the actor runs on, and
    // dropping it here would tear the actor down mid-test.
    std::mem::forget(_backend);
    db
}

async fn view_exists(db: &DbHandle, view_name: &str) -> bool {
    !db.query(
        &format!("SELECT name FROM sqlite_master WHERE type='view' AND name='{view_name}'"),
        HashMap::new(),
    )
    .await
    .expect("probe sqlite_master")
    .is_empty()
}

async fn watch_view_names(db: &DbHandle) -> Vec<String> {
    db.query(
        "SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%' ORDER BY \
         name",
        HashMap::new(),
    )
    .await
    .expect("probe sqlite_master")
    .iter()
    .filter_map(|row| match row.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        _ => None,
    })
    .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_materializes_the_view_and_its_release_reaps_it() {
    let db = booted().await;
    assert_eq!(db.matview_stats().leased_views, 0);

    let grant = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("acquire lease");

    assert!(view_exists(&db, PROBE_VIEW).await);
    let stats = db.matview_stats();
    assert_eq!(
        (stats.leased_views, stats.active_leases, stats.pinned),
        (1, 1, 0)
    );

    db.release_view_lease(PROBE_VIEW, grant);

    assert!(
        !view_exists(&db, PROBE_VIEW).await,
        "the release and the reap are one command, so the next query already sees the view gone"
    );
    let stats = db.matview_stats();
    assert_eq!(
        (stats.leased_views, stats.active_leases, stats.pinned),
        (0, 0, 0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_last_release_reaps() {
    let db = booted().await;

    let first = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("first lease");
    let second = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("second lease");
    assert_ne!(
        first.lease_id, second.lease_id,
        "each grant must be distinguishable"
    );

    let stats = db.matview_stats();
    assert_eq!((stats.leased_views, stats.active_leases), (1, 2));

    db.release_view_lease(PROBE_VIEW, first);
    assert!(
        view_exists(&db, PROBE_VIEW).await,
        "a view with a live lease left must not be reaped"
    );
    assert_eq!(db.matview_stats().active_leases, 1);

    db.release_view_lease(PROBE_VIEW, second);
    assert!(!view_exists(&db, PROBE_VIEW).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_outlives_a_whole_lease_cycle() {
    let db = booted().await;

    db.ensure_pinned_view(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("pin the view");
    assert_eq!(db.matview_stats().pinned, 1);

    let grant = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("lease a pinned view");
    db.release_view_lease(PROBE_VIEW, grant);

    assert!(
        view_exists(&db, PROBE_VIEW).await,
        "a pin is never released, so the lease cycle above cannot reap the view"
    );
    let stats = db.matview_stats();
    assert_eq!(
        (stats.leased_views, stats.active_leases, stats.pinned),
        (1, 0, 1)
    );
}

/// The `Creating` state: leases taken while the `CREATE` is parked on the
/// deferred-DDL queue must all be granted once the base table shows up.
#[tokio::test(flavor = "multi_thread")]
async fn leases_taken_while_the_create_is_parked_are_all_granted() {
    let (_backend, db) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso backend");
    std::mem::forget(_backend);

    let deferred_view = "watch_view_deferred";
    let deferred_select = "SELECT id FROM arrives_later";

    let a = tokio::spawn({
        let db = db.clone();
        async move { db.acquire_view_lease(deferred_view, deferred_select).await }
    });
    let b = tokio::spawn({
        let db = db.clone();
        async move { db.acquire_view_lease(deferred_view, deferred_select).await }
    });

    // Let both acquires reach the actor before the dependency arrives; without
    // this the test could pass by taking the ordinary immediate-create path.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !view_exists(&db, deferred_view).await,
        "the CREATE must stay parked while its base table is missing"
    );

    db.execute_ddl("CREATE TABLE arrives_later (id TEXT PRIMARY KEY)")
        .await
        .expect("create the awaited base table");

    let first = a.await.expect("join a").expect("first lease granted");
    let second = b.await.expect("join b").expect("second lease granted");
    assert_ne!(first.lease_id, second.lease_id);
    assert!(view_exists(&db, deferred_view).await);
    assert_eq!(db.matview_stats().active_leases, 2);

    db.release_view_lease(deferred_view, first);
    db.release_view_lease(deferred_view, second);
    assert!(!view_exists(&db, deferred_view).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_drops_every_watch_view_and_makes_older_grants_inert() {
    let db = booted().await;

    let stale = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("acquire lease");
    assert_eq!(watch_view_names(&db).await, vec![PROBE_VIEW.to_string()]);

    let dropped = db.reset_watch_views().await.expect("reset watch views");
    assert_eq!(dropped, 1);
    assert!(watch_view_names(&db).await.is_empty());
    assert_eq!(db.matview_stats().leased_views, 0);

    // The pre-reset grant now names a view of a bygone generation. Releasing it
    // must not touch the view the next acquire creates.
    db.release_view_lease(PROBE_VIEW, stale);
    let fresh = db
        .acquire_view_lease(PROBE_VIEW, PROBE_SELECT)
        .await
        .expect("re-acquire after reset");
    assert!(view_exists(&db, PROBE_VIEW).await);
    assert_eq!(db.matview_stats().active_leases, 1);

    db.release_view_lease(PROBE_VIEW, fresh);
    assert!(!view_exists(&db, PROBE_VIEW).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unparseable_select_fails_before_it_reaches_the_actor() {
    let db = booted().await;

    let err = db
        .acquire_view_lease(PROBE_VIEW, "SELECT FROM WHERE ((")
        .await
        .expect_err("a malformed SELECT must not be queued as DDL");
    let message = err.to_string();
    assert!(
        message.contains("failed to parse"),
        "the error must name the parse failure, got: {message}"
    );
    assert_eq!(db.matview_stats().leased_views, 0);
}
