//! Contract: a watch over a table NOBODY registers fails FAST and by name.
//!
//! The DDL queue exists so boot-time DDL can be submitted out of order and
//! park until its dependencies arrive. That park is only legitimate while
//! registration is still open (`SchemaInit`). Once the actor is `Ready` the
//! set of resources anyone will ever provide is closed, so a watch whose
//! SELECT names a table nobody promised can only ever wait forever — today it
//! parks for the full 120s dependency timeout and the block it feeds shows
//! "(loading)" the entire time.

use std::sync::Arc;
use std::time::Duration;

use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::TursoBackend;

#[tokio::test]
async fn watch_on_unregistered_table_fails_fast_and_names_it() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    handle
        .execute_ddl("CREATE TABLE known_t (id TEXT PRIMARY KEY, val TEXT)")
        .await
        .expect("create known table");

    // Registration is closed: everything that will ever exist, exists.
    handle
        .transition_to_ready()
        .await
        .expect("transition to ready");

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));

    // `probe_t` is registered by nobody. Waiting for it cannot succeed.
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        mgr.watch("SELECT id, val FROM probe_t"),
    )
    .await
    .expect(
        "watch over an unregistered table PARKED instead of failing fast — this is the \
         120s-hang/'(loading)-forever' bug",
    );

    let Err(err) = outcome else {
        panic!("a watch over a table nobody registers must be an error");
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("probe_t"),
        "the error must name the missing table so the UI can disclose it; got: {msg}"
    );
    assert!(
        !msg.contains("timed out"),
        "the failure must be a named missing-dependency error, not a timeout; got: {msg}"
    );
}

#[tokio::test]
async fn watch_on_registered_table_still_works_after_ready() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    handle
        .execute_ddl("CREATE TABLE known_t (id TEXT PRIMARY KEY, val TEXT)")
        .await
        .expect("create known table");
    handle
        .transition_to_ready()
        .await
        .expect("transition to ready");

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));

    tokio::time::timeout(
        Duration::from_secs(10),
        mgr.watch("SELECT id, val FROM known_t"),
    )
    .await
    .expect("watch over an existing table must not park")
    .expect("watch over an existing table must succeed");
}

/// A CTE name is not a table. The fast-fail reads its missing set from the
/// SQL's table refs, so a post-Ready watch whose query defines a recursive CTE
/// must not mistake the CTE for a table nobody registered — that would reject
/// the page-hierarchy queries the app runs constantly.
#[tokio::test]
async fn recursive_cte_names_are_not_treated_as_unregistered_tables() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    handle
        .execute_ddl("CREATE TABLE node_t (id TEXT PRIMARY KEY, parent_id TEXT)")
        .await
        .expect("create base table");
    handle
        .transition_to_ready()
        .await
        .expect("transition to ready");

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));

    tokio::time::timeout(
        Duration::from_secs(10),
        mgr.watch(
            "WITH RECURSIVE descend AS (SELECT id, parent_id FROM node_t WHERE parent_id IS NULL \
             UNION ALL SELECT n.id, n.parent_id FROM node_t n JOIN descend d ON n.parent_id = \
             d.id) SELECT id, parent_id FROM descend",
        ),
    )
    .await
    .expect("a CTE watch must not park")
    .expect("a CTE name must not be reported as an unregistered table");
}

/// The deferred wait is the whole point during boot: DI resolves schema
/// providers in parallel, so a matview's base table can legitimately arrive
/// after the matview's CREATE is submitted. Fast-fail must not eat that.
#[tokio::test]
async fn ddl_still_waits_for_a_late_dependency_during_schema_init() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let watching = tokio::spawn(async move { mgr.watch("SELECT id, val FROM late_t").await });

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle
        .execute_ddl("CREATE TABLE late_t (id TEXT PRIMARY KEY, val TEXT)")
        .await
        .expect("create the late table");

    tokio::time::timeout(Duration::from_secs(10), watching)
        .await
        .expect("the parked watch must resume once its dependency arrives")
        .expect("watch task panicked")
        .expect("watch must succeed once the late dependency lands");
}

/// The fast-fail only sees ops submitted AFTER Ready. An op parked during
/// `SchemaInit` for a table nobody ever registers is still parked at the
/// moment registration closes, so the Ready transition must sweep the queue —
/// otherwise it burns the full 120s dependency timeout.
#[tokio::test]
async fn ready_transition_fails_ops_parked_on_a_never_registered_table() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let watching =
        tokio::spawn(async move { mgr.watch("SELECT id, val FROM never_registered_t").await });

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle
        .transition_to_ready()
        .await
        .expect("transition to ready");

    let outcome = tokio::time::timeout(Duration::from_secs(2), watching)
        .await
        .expect(
            "the op parked during SchemaInit stayed parked across the Ready transition — this is \
             the 120s-hang bug the fast-fail was built to eliminate",
        )
        .expect("watch task panicked");

    let Err(err) = outcome else {
        panic!("a watch parked on a table nobody registers must be an error once Ready");
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("never_registered_t"),
        "the error must name the missing table; got: {msg}"
    );
    assert!(
        !msg.contains("timed out"),
        "the failure must be a named missing-dependency error, not a timeout; got: {msg}"
    );
}
