//! R2 spike (C2 INC 2): does the fork Turso IVM maintain a GROUP BY + COUNT
//! matview built directly over a PLAIN TABLE?
//!
//! The `automations_journal` matview groups `block_history` by
//! `(origin, transition_id, day)` and counts. That is a matview-over-TABLE, so
//! the chained-matview hang class (skill turso-chained-matview-hang) should not
//! apply — but the conflicting signals in the risk register (R2) demand we
//! prove it before building on it. This spike is the minimal reproduction:
//! create the aggregating matview, mutate the base table, assert IVM maintains
//! the grouped counts O(delta) — inserts increment a group, retract a group to
//! zero on delete, and split into a new group on a distinct key.
//!
//! If this hangs or the counts are wrong, INC 2 stops here and the fork
//! (memory turso-ivm-ours) needs GROUP BY-over-table IVM work.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend); // keep the actor alive for the test
    handle
        .execute_ddl(
            "CREATE TABLE evt (seq INTEGER PRIMARY KEY, origin TEXT NOT NULL, transition_id TEXT, \
             day TEXT NOT NULL)",
        )
        .await
        .expect("create base table");
    handle
}

async fn insert_evt(handle: &DbHandle, seq: i64, origin: &str, transition: &str, day: &str) {
    handle
        .execute(
            "INSERT INTO evt (seq, origin, transition_id, day) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Integer(seq),
                turso::Value::Text(origin.into()),
                turso::Value::Text(transition.into()),
                turso::Value::Text(day.into()),
            ],
        )
        .await
        .expect("insert evt");
}

async fn delete_evt(handle: &DbHandle, seq: i64) {
    handle
        .execute(
            "DELETE FROM evt WHERE seq = ?",
            vec![turso::Value::Integer(seq)],
        )
        .await
        .expect("delete evt");
}

/// Read `(origin, transition_id, day, n)` from the grouped matview, sorted.
async fn read_journal(handle: &DbHandle) -> Vec<(String, String, String, i64)> {
    let rows = handle
        .query(
            "SELECT origin, transition_id, day, n FROM evt_journal ORDER BY origin, \
             transition_id, day",
            HashMap::new(),
        )
        .await
        .expect("query grouped matview");
    let mut out: Vec<(String, String, String, i64)> = rows
        .iter()
        .map(|r| {
            let origin = match r.get("origin") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("origin: unexpected {other:?}"),
            };
            let transition = match r.get("transition_id") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("transition_id: unexpected {other:?}"),
            };
            let day = match r.get("day") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("day: unexpected {other:?}"),
            };
            let n = match r.get("n") {
                Some(Value::Integer(i)) => *i,
                other => panic!("n: unexpected {other:?}"),
            };
            (origin, transition, day, n)
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn group_by_count_matview_over_table_is_ivm_maintained() {
    let handle = setup().await;

    let created = reconcile_named_view(
        &handle,
        "evt_journal",
        "SELECT origin, transition_id, day, COUNT(*) AS n FROM evt GROUP BY origin, \
         transition_id, day",
    )
    .await
    .expect("grouped matview DDL must succeed (no hang)");
    assert!(created, "first reconcile creates the view");

    // Two events in the same (origin, transition, day) group -> count 2.
    insert_evt(&handle, 1, "rule", "t1", "2026-07-16").await;
    insert_evt(&handle, 2, "rule", "t1", "2026-07-16").await;
    assert_eq!(
        read_journal(&handle).await,
        vec![("rule".into(), "t1".into(), "2026-07-16".into(), 2)],
    );

    // A distinct day splits into a new group — IVM adds a second row, does not
    // fold into the first.
    insert_evt(&handle, 3, "rule", "t1", "2026-07-17").await;
    assert_eq!(
        read_journal(&handle).await,
        vec![
            ("rule".into(), "t1".into(), "2026-07-16".into(), 2),
            ("rule".into(), "t1".into(), "2026-07-17".into(), 1),
        ],
    );

    // Delete one of the two rows in the first group — count drops to 1, the
    // group survives (partial retraction).
    delete_evt(&handle, 1).await;
    assert_eq!(
        read_journal(&handle).await,
        vec![
            ("rule".into(), "t1".into(), "2026-07-16".into(), 1),
            ("rule".into(), "t1".into(), "2026-07-17".into(), 1),
        ],
    );

    // Delete the last row of the first group — the whole group is retracted.
    delete_evt(&handle, 2).await;
    assert_eq!(
        read_journal(&handle).await,
        vec![("rule".into(), "t1".into(), "2026-07-17".into(), 1)],
    );
}
