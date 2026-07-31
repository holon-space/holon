//! Regression (#39): a watched query silently lost its `ORDER BY`.
//!
//! `ensure_view` strips the trailing `ORDER BY` because Turso IVM rejects a
//! Sort node in a matview body — but `query_view` then read the view back with
//! a bare `SELECT *`, so a sidecar `sort {-last_activity}` came back in rowid
//! order. A one-shot `execute_query` of the same SQL honours the clause, so
//! watched and unwatched reads of one query disagreed on order.
//!
//! Regression (#86): re-applying that clause VERBATIM broke every query whose
//! `ORDER BY` names a table alias (`ORDER BY b.title`). The alias is in scope
//! in the source query's `FROM`, but the view read is `FROM watch_view_…` —
//! so the re-emitted clause failed with `no such table: b`. GQL compiles the
//! shipped right-sidebar query to exactly that shape.

use std::collections::HashMap;

use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use holon_turso::util::strip_order_by;
use holon_turso::util::trailing_order_by;

const WATCHED_SQL: &str =
    "SELECT session_id, last_activity FROM cc_session ORDER BY last_activity DESC";

async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE cc_session (session_id TEXT PRIMARY KEY, last_activity TEXT)")
        .await
        .expect("create table");
    // Insert in an order that does NOT match the requested sort, so rowid
    // order and `last_activity DESC` order are distinguishable.
    for (sid, ts) in [
        ("s-mid", "2026-07-20T00:00:00Z"),
        ("s-old", "2026-01-01T00:00:00Z"),
        ("s-new", "2026-07-30T00:00:00Z"),
    ] {
        handle
            .execute(
                "INSERT INTO cc_session VALUES (?, ?)",
                vec![
                    turso::Value::Text(sid.into()),
                    turso::Value::Text(ts.into()),
                ],
            )
            .await
            .expect("insert session");
    }
    handle
}

fn session_ids(rows: &[holon_core::storage::StorageEntity]) -> Vec<String> {
    rows.iter()
        .map(|r| match r.get("session_id") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("session_id: unexpected value {other:?}"),
        })
        .collect()
}

#[test]
fn trailing_order_by_returns_the_clause_strip_removes() {
    assert_eq!(
        trailing_order_by(WATCHED_SQL).as_deref(),
        Some("ORDER BY last_activity DESC")
    );
    // LIMIT / OFFSET are NOT part of the clause — the matview holds the
    // unbounded relation and its CDC stream delivers changes beyond a window.
    assert_eq!(
        trailing_order_by("SELECT * FROM t ORDER BY name ASC LIMIT 10 OFFSET 2").as_deref(),
        Some("ORDER BY name ASC")
    );
    assert_eq!(trailing_order_by("SELECT * FROM t LIMIT 10"), None);
    assert_eq!(trailing_order_by("SELECT * FROM t"), None);
    // A nested ORDER BY inside a subquery stays with the body.
    let nested = "SELECT * FROM (SELECT * FROM t ORDER BY x) sub";
    assert_eq!(trailing_order_by(nested), None);
    assert_eq!(strip_order_by(nested), nested);
}

#[tokio::test]
async fn watch_returns_the_snapshot_in_the_queries_declared_order() {
    let handle = setup().await;
    let manager = MatviewManager::new(
        handle.clone(),
        std::sync::Arc::new(tokio::sync::Mutex::new(())),
    );

    let watched = manager.watch(WATCHED_SQL).await.expect("watch");
    assert_eq!(
        session_ids(&watched.initial_rows),
        vec!["s-new", "s-mid", "s-old"],
        "a watched query's snapshot must honour its ORDER BY, not come back in rowid order"
    );

    // ... and it must agree with a one-shot read of the same SQL, which never
    // went through the matview strip.
    let direct = handle
        .query(WATCHED_SQL, HashMap::new())
        .await
        .expect("direct query");
    assert_eq!(
        session_ids(&watched.initial_rows),
        session_ids(&direct),
        "watched and unwatched reads of one query must agree on order"
    );
}

// ---------------------------------------------------------------------------
// #86 — alias-qualified ORDER BY through a watched query.
// ---------------------------------------------------------------------------

/// Shape-faithful to the shipped right-sidebar query: a join whose trailing
/// `ORDER BY` qualifies its columns with the source `FROM` aliases.
const ALIASED_SQL: &str = "SELECT b.id, b.title, d.name FROM blk b JOIN doc d ON d.id = b.doc \
                           ORDER BY d.name ASC, b.title DESC";

async fn setup_aliased() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE doc (id TEXT PRIMARY KEY, name TEXT)")
        .await
        .expect("create doc");
    handle
        .execute_ddl("CREATE TABLE blk (id TEXT PRIMARY KEY, title TEXT, doc TEXT)")
        .await
        .expect("create blk");
    for (id, name) in [("d1", "alpha"), ("d2", "beta")] {
        handle
            .execute(
                "INSERT INTO doc VALUES (?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(name.into()),
                ],
            )
            .await
            .expect("insert doc");
    }
    // Insertion order deliberately disagrees with `d.name ASC, b.title DESC`.
    for (id, title, doc) in [
        ("b1", "aaa", "d2"),
        ("b2", "zzz", "d1"),
        ("b3", "mmm", "d1"),
        ("b4", "yyy", "d2"),
    ] {
        handle
            .execute(
                "INSERT INTO blk VALUES (?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(title.into()),
                    turso::Value::Text(doc.into()),
                ],
            )
            .await
            .expect("insert blk");
    }
    handle
}

fn ids(rows: &[holon_core::storage::StorageEntity]) -> Vec<String> {
    rows.iter()
        .map(|r| match r.get("id") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("id: unexpected value {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn watch_honours_an_alias_qualified_order_by() {
    let handle = setup_aliased().await;
    let manager = MatviewManager::new(
        handle.clone(),
        std::sync::Arc::new(tokio::sync::Mutex::new(())),
    );

    let watched = manager.watch(ALIASED_SQL).await.expect(
        "watching a query whose ORDER BY names a source table alias must not fail: the alias is \
         not in scope over the view, so the clause has to be rewritten onto the view's output \
         columns",
    );

    let direct = handle
        .query(ALIASED_SQL, HashMap::new())
        .await
        .expect("direct query");
    assert_eq!(
        ids(&direct),
        vec!["b2", "b3", "b4", "b1"],
        "sanity: the unwatched read establishes the declared order"
    );
    assert_eq!(
        ids(&watched.initial_rows),
        ids(&direct),
        "watched and unwatched reads of one aliased query must agree on order"
    );
}

/// The SNEAKIER half of #86: even where the alias-qualified clause does not
/// crash the read, it silently loses the declared order.
/// `BackendEngine::query_ordering_spec` (`backend_engine.rs:327`) feeds this
/// clause to `order_by_sort_spec`, whose plain-column check rejects the dot in
/// `b.title` — so the collection renders in its default order with only a
/// warn-level trace. The rendered rows are the VIEW's, whose column carries the
/// bare name, so the qualifier must be dropped rather than treated as an
/// inexpressible expression.
#[test]
fn sort_spec_sees_through_a_table_alias() {
    assert_eq!(
        holon_turso::util::order_by_sort_spec("ORDER BY b.title").as_deref(),
        Some("title")
    );
    assert_eq!(
        holon_turso::util::order_by_sort_spec("ORDER BY fr.added_ts DESC").as_deref(),
        Some("-added_ts")
    );
    assert_eq!(
        holon_turso::util::order_by_sort_spec("ORDER BY b.\"title\" DESC").as_deref(),
        Some("-title")
    );
    // Still NOT expressible: a qualified column inside an expression.
    assert_eq!(
        holon_turso::util::order_by_sort_spec("ORDER BY lower(b.title)"),
        None
    );
}
