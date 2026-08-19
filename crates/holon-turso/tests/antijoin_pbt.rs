//! Differential IVM property: a materialized view's IVM-maintained rows must
//! equal a FRESH re-execution of its own defining SELECT after every mutation.
//!
//! This is `inv-matview-consistent-with-recompute` applied at the engine tier,
//! with a generator whose shapes MUST include the correlated `NOT EXISTS`
//! anti-join and `OR(EXISTS, NOT EXISTS)` that Martin's `Now.org` planning
//! query uses — the shapes the fork's DBSP IVM cannot maintain.
//!
//! The DB is the faithful prod shape: `block_raw` + per-junction aggregation
//! matviews (`block_requires_agg`, `block_tags_agg`) + the chained `block`
//! matview, with a `watch_view` matview on top (exactly what `query_and_watch`
//! builds for a live_query).
//!
//! The trigger for the SILENT (vs loud) refusal was a COMPUTED conjunct beside
//! the subquery — Now.org's leading `json_extract(...)='TODO' AND NOT EXISTS
//! (…)` — not chaining (turso-6f 8-shape bisect; corroborated here). The
//! projection rewrite aliased the subquery onto a shared `__temp_filter_expr`
//! temp column, so CREATE succeeded with an always-false filter.
//!
//! Result matrix (this file is the executable record):
//!   * single `json_extract` property filter              -> IVM-maintained  ✅
//!     green (`prop_matview_consistent_maintainable`)
//!   * `json_extract` conjunct + correlated `NOT EXISTS`  -> REJECTED LOUDLY at
//!     DDL, in both the isolated and the full Now.org shape
//!     (`antijoin_isolated_create_refuses_loudly`,
//!     `now_org_antijoin_create_refuses_loudly_after_fix`). See
//!     docs/Testing/bugfunnel/entries/
//!     2026-08-19-ivm-antijoin-matview-silently-empty.md
//!   * a PLAIN-column conjunct + `NOT EXISTS` over a base table -> REJECTED
//!     LOUDLY at DDL. Pinned green (`base_table_antijoin_ddl_rejected`).
//!
//! The fork does NOT maintain `EXISTS`/anti-joins — the bypass fix made the
//! refusal LOUD in ALL conjunct combinations, so the anti-join cases assert
//! refusal, not matview≡fresh. The render path is unaffected either way:
//! `sql_ivm_maintainable` routes the shape eager BEFORE any CREATE.
//!
//! Now.org rewrite option: the readiness clause CAN be rewritten to a
//! maintainable `LEFT JOIN … IS NULL` — VERIFIED correct on the landed populate
//! fix `c6cfab7d` (served == fresh, end-to-end pin
//! `left_join_isnull_matview_matches_fresh_after_populate_fix` in
//! backend_engine.rs). It was NOT correct before that pin (over-served,
//! undisclosed), so eager + disclosure stays the honest default and the rewrite
//! is a later optimization, not a workaround to reach for on an older engine.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;

// --- Harness ---------------------------------------------------------------

/// Stand up the faithful prod block schema on a fresh in-memory db.
async fn block_schema() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, properties TEXT NOT NULL DEFAULT '{}')",
        )
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE block_requires (block_id TEXT, required_id TEXT)")
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE block_tags (block_id TEXT, tag TEXT)")
        .await
        .unwrap();
    reconcile_named_view(
        &handle,
        "block_requires_agg",
        "SELECT block_id AS source_id, json_group_array(required_id) AS vals FROM block_requires \
         GROUP BY block_id",
    )
    .await
    .unwrap();
    reconcile_named_view(
        &handle,
        "block_tags_agg",
        "SELECT block_id AS source_id, json_group_array(tag) AS vals FROM block_tags GROUP BY \
         block_id",
    )
    .await
    .unwrap();
    reconcile_named_view(
        &handle,
        "block",
        "SELECT b.id, b.properties, COALESCE(rq.vals,'[]') AS requires, COALESCE(tg.vals,'[]') AS \
         tags FROM block_raw b LEFT OUTER JOIN block_requires_agg rq ON rq.source_id = b.id LEFT \
         OUTER JOIN block_tags_agg tg ON tg.source_id = b.id WHERE b.id != 'sentinel:no_parent'",
    )
    .await
    .unwrap();
    handle
}

/// A single base-table mutation the generator can emit. Touches BOTH the outer
/// rows (`block_raw` properties) AND the correlated tables (`block_requires`,
/// `block_tags`) — the anti-join's correctness depends on deltas from all
/// three.
#[derive(Debug, Clone)]
enum Mutation {
    SetBlock {
        id: u8,
        task_state: &'static str,
        gate: &'static str,
    },
    DeleteBlock {
        id: u8,
    },
    AddRequires {
        block: u8,
        required: u8,
    },
    RemoveRequires {
        block: u8,
        required: u8,
    },
    AddTag {
        block: u8,
        tag: &'static str,
    },
    RemoveTag {
        block: u8,
        tag: &'static str,
    },
}

fn bid(id: u8) -> String {
    format!("b{id}")
}

async fn apply(handle: &DbHandle, m: &Mutation) {
    match m {
        Mutation::SetBlock {
            id,
            task_state,
            gate,
        } => {
            let props = format!("{{\"task_state\":\"{task_state}\",\"gate\":\"{gate}\"}}");
            handle
                .execute(
                    "INSERT INTO block_raw (id, properties) VALUES (?, ?) ON CONFLICT(id) DO \
                     UPDATE SET properties = excluded.properties",
                    vec![turso::Value::Text(bid(*id)), turso::Value::Text(props)],
                )
                .await
                .unwrap();
        }
        Mutation::DeleteBlock { id } => {
            handle
                .execute(
                    "DELETE FROM block_raw WHERE id = ?",
                    vec![turso::Value::Text(bid(*id))],
                )
                .await
                .unwrap();
        }
        Mutation::AddRequires { block, required } => {
            handle
                .execute(
                    "INSERT INTO block_requires (block_id, required_id) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(bid(*block)),
                        turso::Value::Text(bid(*required)),
                    ],
                )
                .await
                .unwrap();
        }
        Mutation::RemoveRequires { block, required } => {
            handle
                .execute(
                    "DELETE FROM block_requires WHERE block_id = ? AND required_id = ?",
                    vec![
                        turso::Value::Text(bid(*block)),
                        turso::Value::Text(bid(*required)),
                    ],
                )
                .await
                .unwrap();
        }
        Mutation::AddTag { block, tag } => {
            handle
                .execute(
                    "INSERT INTO block_tags (block_id, tag) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(bid(*block)),
                        turso::Value::Text((*tag).into()),
                    ],
                )
                .await
                .unwrap();
        }
        Mutation::RemoveTag { block, tag } => {
            handle
                .execute(
                    "DELETE FROM block_tags WHERE block_id = ? AND tag = ?",
                    vec![
                        turso::Value::Text(bid(*block)),
                        turso::Value::Text((*tag).into()),
                    ],
                )
                .await
                .unwrap();
        }
    }
}

/// Sorted `id`s of a SELECT executed FRESH (bypassing every matview cache).
async fn recompute_ids(handle: &DbHandle, select: &str) -> Vec<String> {
    row_ids(handle, select).await
}

/// Sorted `id`s a matview currently serves.
async fn matview_ids(handle: &DbHandle, view: &str) -> Vec<String> {
    row_ids(handle, &format!("SELECT id FROM {view}")).await
}

async fn row_ids(handle: &DbHandle, sql: &str) -> Vec<String> {
    let rows = handle.query(sql, HashMap::new()).await.unwrap();
    let mut ids: Vec<String> = rows
        .iter()
        .map(|r| match r.get("id") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("id: unexpected {other:?}"),
        })
        .collect();
    ids.sort();
    ids
}

/// Build a watch matview for `select`, apply `muts`, settle, and return
/// `(matview_ids, fresh_recompute_ids)`. The differential the property checks.
async fn drive(select: &str, muts: &[Mutation]) -> (Vec<String>, Vec<String>) {
    let handle = block_schema().await;
    reconcile_named_view(&handle, "watch_view_prop", select)
        .await
        .expect("watch matview create");
    for m in muts {
        apply(&handle, m).await;
    }
    // Let IVM maintenance settle.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let mv = matview_ids(&handle, "watch_view_prop").await;
    let fresh = recompute_ids(&handle, select).await;
    (mv, fresh)
}

// The defining SELECTs, all over the chained `block` matview.

/// Maintainable control: a SINGLE `json_extract` property filter — the shape
/// IVM handles (empirically green; see the probe matrix in the module doc).
/// A single predicate deliberately: a SECOND `json_extract` AND-predicate is a
/// SEPARATE fork maintenance bug (bugfunnel
/// 2026-08-19-ivm-two-json-extract-predicates-matview-empty), which would
/// otherwise confound this control with the anti-join under test.
const SIMPLE_FILTER: &str =
    "SELECT b.id FROM block b WHERE json_extract(b.properties,'$.task_state') = 'TODO'";

/// Anti-join under test, ISOLATED: one `json_extract` (maintainable on its own,
/// per `SIMPLE_FILTER`) plus the correlated `NOT EXISTS` wrapping an inner
/// JOIN. The ONLY un-maintainable element vs `SIMPLE_FILTER` is the anti-join,
/// so a divergence here attributes cleanly to it.
const ANTIJOIN_ISOLATED: &str = "SELECT b.id FROM block b WHERE \
    json_extract(b.properties,'$.task_state') = 'TODO' AND \
    NOT EXISTS (SELECT 1 FROM block_requires br JOIN block bl ON bl.id = br.required_id \
        WHERE br.block_id = b.id AND COALESCE(json_extract(bl.properties,'$.task_state'),'') != 'DONE')";

/// The FULL Now.org shape: correlated NOT EXISTS anti-join (wrapping an inner
/// JOIN) plus OR(EXISTS, NOT EXISTS). The faithful regression witness.
const ANTIJOIN: &str = "SELECT b.id FROM block b WHERE \
    json_extract(b.properties,'$.task_state') = 'TODO' AND \
    json_extract(b.properties,'$.gate') = 'G1' AND \
    NOT EXISTS (SELECT 1 FROM block_requires br JOIN block bl ON bl.id = br.required_id \
        WHERE br.block_id = b.id AND COALESCE(json_extract(bl.properties,'$.task_state'),'') != 'DONE') AND \
    (EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'agent') \
     OR NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only'))";

// --- Generators ------------------------------------------------------------

fn mutation_strategy() -> impl Strategy<Value = Mutation> {
    let id = 0u8..6u8;
    prop_oneof![
        (
            id.clone(),
            prop_oneof![Just("TODO"), Just("DONE")],
            prop_oneof![Just("G1"), Just("G2")]
        )
            .prop_map(|(id, task_state, gate)| Mutation::SetBlock {
                id,
                task_state,
                gate
            }),
        id.clone().prop_map(|id| Mutation::DeleteBlock { id }),
        (id.clone(), id.clone())
            .prop_map(|(block, required)| Mutation::AddRequires { block, required }),
        (id.clone(), id.clone())
            .prop_map(|(block, required)| Mutation::RemoveRequires { block, required }),
        (id.clone(), prop_oneof![Just("agent"), Just("human-only")])
            .prop_map(|(block, tag)| Mutation::AddTag { block, tag }),
        (id, prop_oneof![Just("agent"), Just("human-only")])
            .prop_map(|(block, tag)| Mutation::RemoveTag { block, tag }),
    ]
}

fn mutations_strategy() -> impl Strategy<Value = Vec<Mutation>> {
    prop::collection::vec(mutation_strategy(), 1..10)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// --- Properties ------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// GREEN: for a maintainable shape the matview equals the recompute after
    /// any mutation sequence — proves the differential harness itself detects
    /// agreement and that simple filters ARE incrementally maintained.
    #[test]
    fn prop_matview_consistent_maintainable(muts in mutations_strategy()) {
        let (mv, fresh) = runtime().block_on(drive(SIMPLE_FILTER, &muts));
        prop_assert_eq!(mv, fresh, "simple-filter matview must equal fresh recompute");
    }
}

/// The anti-join ISOLATED: one maintainable `json_extract` conjunct plus the
/// correlated `NOT EXISTS`. The anti-join is the only un-maintainable element
/// vs `SIMPLE_FILTER`, so the refusal attributes cleanly to it.
#[tokio::test]
async fn antijoin_isolated_create_refuses_loudly() {
    let handle = block_schema().await;
    let result = reconcile_named_view(&handle, "watch_view_isolated_loud", ANTIJOIN_ISOLATED).await;
    assert!(
        result.is_err(),
        "a computed conjunct beside a correlated NOT EXISTS must refuse LOUDLY at CREATE, not \
         silently succeed-empty; got {result:?}"
    );
}

/// The FULL Now.org shape: anti-join plus `OR(EXISTS, NOT EXISTS)`. The CREATE
/// that once succeeded-then-served-0-rows now ERRORs.
#[tokio::test]
async fn now_org_antijoin_create_refuses_loudly_after_fix() {
    let handle = block_schema().await;
    let result = reconcile_named_view(&handle, "watch_view_now_loud", ANTIJOIN).await;
    assert!(
        result.is_err(),
        "post-fix the computed-conjunct + subquery matview CREATE must refuse LOUDLY, not \
         silently succeed-empty; got {result:?}"
    );
}

/// GREEN: the SAME anti-join over a BASE TABLE is rejected at DDL — the fork's
/// AST conversion cannot lower `NOT EXISTS`. Pins the other failure mode.
#[tokio::test]
async fn base_table_antijoin_ddl_rejected() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE blk (id TEXT PRIMARY KEY, task_state TEXT)")
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE blk_requires (block_id TEXT, required_id TEXT)")
        .await
        .unwrap();
    let select = "SELECT b.id FROM blk b WHERE NOT EXISTS (SELECT 1 FROM blk_requires br JOIN blk \
                  bl ON bl.id = br.required_id WHERE br.block_id = b.id AND \
                  COALESCE(bl.task_state,'') != 'DONE')";
    let result = reconcile_named_view(&handle, "aj_base", select).await;
    assert!(
        result.is_err(),
        "base-table correlated NOT EXISTS must be rejected at matview DDL; got {result:?}"
    );
}
