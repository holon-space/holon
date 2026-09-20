//! Can a membership row PER WATCH be deduplicated by a view the fork's IVM
//! maintains INCREMENTALLY?
//!
//! One row per watch is the shape that makes ownership a fact of the database:
//! each guard deletes its own `(watch_key, nonce)` row and no process-global
//! bookkeeping decides who is last. The whole question is whether the shared
//! view can then still emit each subtree ONCE — two owners of one place must
//! not double the view's output, and the closure must survive the first owner
//! leaving and retract when the last one does.
//!
//! Acceptance of the DDL proves nothing: this fork accepts shapes it does not
//! maintain. Every step below therefore compares the matview against a
//! RECOMPUTE of its own SELECT, which is the same oracle as
//! `inv-matview-consistent-with-recompute`.
//!
//! WHAT THIS FILE DOES NOT PIN. In the running app, a `DISTINCT` seed in the
//! REAL descendant view empties it: `watch_view_ecc943b2177e083e` held 0 rows
//! against a recompute of 5 (lane-logs/v10-hand-authored.log), and the same
//! run with the `DISTINCT` removed and nothing else changed has zero
//! occurrences of that invariant (lane-logs/v11-hand-authored-nodistinct.log).
//! Three isolated attempts to reproduce it here — the simple shape,
//! production's shape, and production's shape under the app's membership
//! churn — all stay CONSISTENT. So the fork limitation that decided this
//! design is evidenced by the gate A/B, not by a test here, and a minimal
//! reproducer for the fork is OWED:
//! lane-logs/repro-distinct-recursive-matview-drift.sql records the shape, the
//! expected/actual, and what the harness is still missing.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// `r` → `c1`, `c2`; `c1` → `g1`. Four blocks, two levels under the root.
async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend); // keep the actor alive for the test
    handle
        .execute_ddl(
            "CREATE TABLE blk (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, content TEXT NOT \
             NULL)",
        )
        .await
        .expect("create block table");
    handle
        .execute_ddl(
            "CREATE TABLE watch_context (watch_key TEXT NOT NULL, nonce TEXT NOT NULL, \
             context_id TEXT NOT NULL, kind TEXT NOT NULL, PRIMARY KEY (watch_key, nonce))",
        )
        .await
        .expect("create membership table");
    for (id, parent) in [
        ("r", "sentinel"),
        ("c1", "r"),
        ("c2", "r"),
        ("g1", "c1"),
        ("other", "sentinel"),
    ] {
        handle
            .execute(
                "INSERT INTO blk (id, parent_id, content) VALUES (?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(format!("{id} content")),
                ],
            )
            .await
            .expect("insert block");
    }
    handle
}

async fn claim(handle: &DbHandle, watch_key: &str, nonce: &str, context_id: &str, kind: &str) {
    handle
        .execute(
            "INSERT INTO watch_context (watch_key, nonce, context_id, kind) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Text(watch_key.into()),
                turso::Value::Text(nonce.into()),
                turso::Value::Text(context_id.into()),
                turso::Value::Text(kind.into()),
            ],
        )
        .await
        .expect("insert claim");
}

/// What a guard issues under the per-watch shape: its OWN row, nothing else.
async fn release(handle: &DbHandle, watch_key: &str, nonce: &str) {
    handle
        .execute(
            "DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?",
            vec![
                turso::Value::Text(watch_key.into()),
                turso::Value::Text(nonce.into()),
            ],
        )
        .await
        .expect("delete claim");
}

async fn rows(handle: &DbHandle, sql: &str) -> Vec<(String, String)> {
    let found = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("query `{sql}`: {e}"));
    let mut out: Vec<(String, String)> = found
        .iter()
        .map(|r| {
            let key = match r.get("watch_key") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("watch_key: unexpected {other:?}"),
            };
            let id = match r.get("id") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("id: unexpected {other:?}"),
            };
            (key, id)
        })
        .collect();
    out.sort();
    out
}

/// The matview's content, and the recompute of its own SELECT, at one moment.
///
/// Returned together because the interesting failure is the two disagreeing:
/// a view the engine accepts but does not maintain drifts from its recompute
/// after a delta, not at creation.
async fn maintained_and_recomputed(
    handle: &DbHandle,
    view: &str,
    select_sql: &str,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    (
        rows(handle, &format!("SELECT watch_key, id FROM {view}")).await,
        rows(handle, &format!("SELECT watch_key, id FROM ({select_sql})")).await,
    )
}

/// The lifecycle every candidate shape has to survive: a second owner of one
/// place must not duplicate the output, the first owner leaving must not
/// retract it, the last owner leaving must, and a re-open must bring it back.
async fn assert_shared_place_lifecycle(
    handle: &DbHandle,
    view: &str,
    select_sql: &str,
    expected_closure: &[(&str, &str)],
) {
    let expected: Vec<(String, String)> = expected_closure
        .iter()
        .map(|(k, id)| ((*k).to_string(), (*id).to_string()))
        .collect();

    let step = async |handle: &DbHandle, label: &str, want: &Vec<(String, String)>| {
        let (maintained, recomputed) = maintained_and_recomputed(handle, view, select_sql).await;
        assert_eq!(
            maintained, recomputed,
            "[{view}] {label}: the matview drifted from a recompute of its own SELECT"
        );
        assert_eq!(
            &maintained, want,
            "[{view}] {label}: wrong rows (maintained)"
        );
    };

    claim(handle, "root:r", "n1", "r", "root").await;
    step(handle, "one owner", &expected).await;

    claim(handle, "root:r", "n2", "r", "root").await;
    step(handle, "a second owner of the same place", &expected).await;

    release(handle, "root:r", "n1").await;
    step(handle, "the first owner left", &expected).await;

    release(handle, "root:r", "n2").await;
    step(handle, "the last owner left", &Vec::new()).await;

    claim(handle, "root:r", "n3", "r", "root").await;
    step(handle, "re-opened", &expected).await;
}

/// Candidate 1: the shared view deduplicates the membership itself, with a
/// `DISTINCT` in the seed.
#[tokio::test]
async fn distinct_in_the_seed_is_incrementally_maintained() {
    let handle = setup().await;
    let select_sql = "WITH RECURSIVE subtree(watch_key, node_id, depth) AS ( \
         SELECT DISTINCT wc.watch_key, b.id, 0 FROM watch_context wc \
         JOIN blk b ON b.id = wc.context_id WHERE wc.kind = 'root' \
         UNION ALL \
         SELECT subtree.watch_key, child.id, subtree.depth + 1 FROM subtree \
         JOIN blk child ON child.parent_id = subtree.node_id WHERE subtree.depth < 10 \
       ) SELECT subtree.watch_key AS watch_key, d.id AS id FROM subtree \
         JOIN blk d ON d.id = subtree.node_id";
    reconcile_named_view(&handle, "wc_distinct_seed", select_sql)
        .await
        .expect("the DDL must be accepted");

    assert_shared_place_lifecycle(
        &handle,
        "wc_distinct_seed",
        select_sql,
        &[
            ("root:r", "c1"),
            ("root:r", "c2"),
            ("root:r", "g1"),
            ("root:r", "r"),
        ],
    )
    .await;
}

/// Candidate 2, REJECTED by measurement: a tiny `GROUP BY` key relation the
/// shared view joins. The engine drops the whole group when ONE of two rows
/// under a key is deleted, and the descendant view chained onto it goes empty
/// while a live watch still holds the place.
///
/// Asserted as observed, so the defect is pinned rather than remembered. If
/// this test ever reds, the engine maintains grouped retraction and the
/// membership can carry per-key columns the `DISTINCT` shape cannot.
#[tokio::test]
async fn a_grouped_key_matview_loses_the_group_on_a_partial_delete() {
    let handle = setup().await;
    reconcile_named_view(
        &handle,
        "watch_keys",
        "SELECT watch_key, MIN(context_id) AS context_id, MIN(kind) AS kind FROM watch_context \
         GROUP BY watch_key",
    )
    .await
    .expect("the key relation's DDL must be accepted");

    let select_sql = "WITH RECURSIVE subtree(watch_key, node_id, depth) AS ( \
         SELECT k.watch_key, b.id, 0 FROM watch_keys k \
         JOIN blk b ON b.id = k.context_id WHERE k.kind = 'root' \
         UNION ALL \
         SELECT subtree.watch_key, child.id, subtree.depth + 1 FROM subtree \
         JOIN blk child ON child.parent_id = subtree.node_id WHERE subtree.depth < 10 \
       ) SELECT subtree.watch_key AS watch_key, d.id AS id FROM subtree \
         JOIN blk d ON d.id = subtree.node_id";
    reconcile_named_view(&handle, "wc_grouped_keys", select_sql)
        .await
        .expect("the descendant view's DDL must be accepted");

    let closure = vec![
        ("root:r".to_string(), "c1".to_string()),
        ("root:r".to_string(), "c2".to_string()),
        ("root:r".to_string(), "g1".to_string()),
        ("root:r".to_string(), "r".to_string()),
    ];
    let view_rows = async |h: &DbHandle| rows(h, "SELECT watch_key, id FROM wc_grouped_keys").await;

    claim(&handle, "root:r", "n1", "r", "root").await;
    claim(&handle, "root:r", "n2", "r", "root").await;
    assert_eq!(
        view_rows(&handle).await,
        closure,
        "two owners of one place must not duplicate the closure"
    );

    release(&handle, "root:r", "n1").await;
    assert_eq!(
        view_rows(&handle).await,
        Vec::new(),
        "MEASUREMENT: the grouped key relation drops the group on a partial delete. Going green \
         here means the engine now maintains grouped retraction."
    );
}

/// Candidate 3: the leaf shape as production writes it — `b.*`, no recursion
/// to hide behind.
#[tokio::test]
async fn the_leaf_shape_deduplicates_with_distinct() {
    let handle = setup().await;
    let select_sql = "SELECT DISTINCT b.*, wc.watch_key AS watch_key FROM blk b \
         JOIN watch_context wc ON b.id = wc.context_id WHERE wc.kind = 'root'";
    reconcile_named_view(&handle, "wc_leaf_distinct", select_sql)
        .await
        .expect("the DDL must be accepted");

    assert_shared_place_lifecycle(&handle, "wc_leaf_distinct", select_sql, &[("root:r", "r")])
        .await;
}

/// A NEGATIVE result, recorded so it is not re-attempted: production's own
/// shape — `d.*`, the `Page` boundary join, the cycle-guard string — does
/// NOT reproduce the drift here.
///
/// In the running app a `DISTINCT` seed empties the descendant view against
/// its own recompute (`inv-matview-consistent-with-recompute`,
/// lane-logs/v10-hand-authored.log; isolated to the `DISTINCT` by
/// lane-logs/v11-hand-authored-nodistinct.log, where the same run is clean
/// without it). Something this harness lacks — the real column set, the
/// concurrent CDC traffic, the chained views over `block` — is part of the
/// trigger, so the keystone gate, not this file, is the authority on that
/// shape.
#[tokio::test]
async fn the_production_descendant_shape_in_isolation_does_not_reproduce_the_drift() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE blk_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL)")
        .await
        .expect("create tag table");
    let select_sql = "WITH RECURSIVE subtree(watch_key, node_id, depth, visited) AS ( \
         SELECT DISTINCT wc.watch_key, b.id, 0, CAST(b.id AS TEXT) \
         FROM watch_context wc JOIN blk b ON b.id = wc.context_id \
         WHERE wc.kind = 'root' \
         UNION ALL \
         SELECT subtree.watch_key, child.id, subtree.depth + 1, \
                subtree.visited || ',' || CAST(child.id AS TEXT) \
         FROM subtree \
         JOIN blk child ON child.parent_id = subtree.node_id \
         LEFT JOIN blk_tags pt ON pt.block_id = subtree.node_id AND pt.tag = 'Page' \
         WHERE subtree.depth < 20 \
           AND ',' || subtree.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%' \
           AND (subtree.depth = 0 OR pt.block_id IS NULL) \
       ) SELECT d.id AS id, subtree.watch_key AS watch_key FROM subtree \
         JOIN blk d ON d.id = subtree.node_id";
    reconcile_named_view(&handle, "wc_prod_distinct", select_sql)
        .await
        .expect("the DDL must be accepted");

    // The app's sequence, not just one claim: a place is registered,
    // re-registered by every re-render (`INSERT OR REPLACE`), released, and
    // taken again — all while the view is maintained.
    claim(&handle, "root:r", "n1", "r", "root").await;
    for nonce in ["n2", "n3", "n4"] {
        handle
            .execute(
                "INSERT OR REPLACE INTO watch_context (watch_key, nonce, context_id, kind) \
                 VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text("root:r".into()),
                    turso::Value::Text(nonce.into()),
                    turso::Value::Text("r".into()),
                    turso::Value::Text("root".into()),
                ],
            )
            .await
            .expect("re-register the place");
        let (maintained, recomputed) =
            maintained_and_recomputed(&handle, "wc_prod_distinct", select_sql).await;
        assert_eq!(
            maintained, recomputed,
            "after re-registering under {nonce}: the isolated reproduction is expected to stay \
             CONSISTENT; the drift the app shows needs more than this harness supplies"
        );
    }
    release(&handle, "root:r", "n4").await;
    claim(&handle, "root:r", "n5", "r", "root").await;
    let (maintained, recomputed) =
        maintained_and_recomputed(&handle, "wc_prod_distinct", select_sql).await;
    assert_eq!(
        maintained, recomputed,
        "after a release and a re-take: the isolated reproduction is expected to stay CONSISTENT; \
         the drift the app shows needs more than this harness supplies"
    );
}
