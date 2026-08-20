//! Differential IVM property: a materialized view's IVM-maintained rows must
//! equal a FRESH re-execution of its own defining SELECT after every mutation.
//!
//! This is `inv-matview-consistent-with-recompute` applied at the engine tier,
//! with a generator whose shapes MUST include the correlated `NOT EXISTS`
//! anti-join and `OR(EXISTS, NOT EXISTS)` that Martin's `Now.org` planning
//! query uses.
//!
//! The DB is the faithful prod shape: `block_raw` + per-junction aggregation
//! matviews (`block_requires_agg`, `block_tags_agg`) + the chained `block`
//! matview, with a `watch_view` matview on top (exactly what `query_and_watch`
//! builds for a live_query).
//!
//! Result matrix (this file is the executable record):
//!   * single `json_extract` property filter -> IVM-maintained
//!     (`prop_matview_consistent_maintainable`)
//!   * `json_extract` conjunct + correlated `NOT EXISTS` -> IVM-maintained, in
//!     both the isolated and the full Now.org shape
//!     (`prop_matview_consistent_antijoin_isolated`,
//!     `prop_matview_consistent_now_org`)
//!   * a PLAIN-column conjunct + `NOT EXISTS` over base tables ->
//!     IVM-maintained (`base_table_antijoin_matview_matches_fresh`)
//!
//! The engine de-correlates `EXISTS` subqueries into indicator anti-joins and
//! gives each distinct computed conjunct its own temp column, so these shapes
//! are maintained rather than refused; `prop_matview_consistent_now_org` is the
//! acceptance pin for that support. Boundaries that still refuse LOUDLY at DDL
//! (never wrong rows): non-equality correlation, uncorrelated `EXISTS`,
//! foreign-table subquery sources, both-sides-complex comparisons.
//!
//! The render path does NOT yet exploit this: `sql_ivm_maintainable` routes
//! every `Exists`/`InSubquery` shape eager BEFORE any CREATE, which is now
//! over-conservative but still correct. Widening it is a separate change with
//! its own red-first test.
//!
//! History: these anti-join cases previously asserted LOUD REFUSAL, and before
//! that shape's bypass fix the CREATE silently succeeded with an always-false
//! filter (0 rows). See docs/Testing/bugfunnel/entries/
//! 2026-08-19-ivm-antijoin-matview-silently-empty.md.

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
/// JOIN. The anti-join is the ONLY element added vs `SIMPLE_FILTER`, so a
/// divergence here attributes cleanly to it.
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

    /// The anti-join ISOLATED: one `json_extract` conjunct plus the correlated
    /// `NOT EXISTS`. The anti-join is the only element beyond `SIMPLE_FILTER`,
    /// so a divergence attributes cleanly to the de-correlated indicator
    /// anti-join.
    #[test]
    fn prop_matview_consistent_antijoin_isolated(muts in mutations_strategy()) {
        let (mv, fresh) = runtime().block_on(drive(ANTIJOIN_ISOLATED, &muts));
        prop_assert_eq!(mv, fresh, "isolated anti-join matview must equal fresh recompute");
    }

    /// ACCEPTANCE PIN for the engine's correlated-EXISTS IVM support: the FULL
    /// Now.org readiness shape is maintained as a LIVE matview, and its served
    /// rows equal a fresh recompute after any mutation sequence.
    #[test]
    fn prop_matview_consistent_now_org(muts in mutations_strategy()) {
        let (mv, fresh) = runtime().block_on(drive(ANTIJOIN, &muts));
        prop_assert_eq!(mv, fresh, "Now.org-shape matview must equal fresh recompute");
    }
}

/// GREEN: the SAME anti-join over BASE TABLES (plain columns, no chained
/// matview underneath) is maintained too, and its rows track base-table deltas.
#[tokio::test]
async fn base_table_antijoin_matview_matches_fresh() {
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
    reconcile_named_view(&handle, "aj_base", select)
        .await
        .expect("base-table correlated NOT EXISTS matview create");

    for (id, state) in [("a", "TODO"), ("b", "TODO"), ("c", "DONE")] {
        handle
            .execute(
                "INSERT INTO blk (id, task_state) VALUES (?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(state.into()),
                ],
            )
            .await
            .unwrap();
    }
    // `a` blocked by the unfinished `b`; `b` cleared by the finished `c`.
    for (block, required) in [("a", "b"), ("b", "c")] {
        handle
            .execute(
                "INSERT INTO blk_requires (block_id, required_id) VALUES (?, ?)",
                vec![
                    turso::Value::Text(block.into()),
                    turso::Value::Text(required.into()),
                ],
            )
            .await
            .unwrap();
    }
    // Unblock `a` by finishing `b` — the delta the anti-join must propagate.
    handle
        .execute("UPDATE blk SET task_state = 'DONE' WHERE id = 'b'", vec![])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let mv = matview_ids(&handle, "aj_base").await;
    let fresh = recompute_ids(&handle, select).await;
    assert_eq!(
        mv, fresh,
        "base-table anti-join matview must equal fresh recompute"
    );
}

/// Non-vacuity + populate pin for the Now.org shape: a deterministic seed whose
/// readiness set is NON-EMPTY, so the differential properties above cannot pass
/// by comparing two empty results.
#[tokio::test]
async fn now_org_matview_populates_non_empty_and_matches_fresh() {
    let muts = vec![
        Mutation::SetBlock {
            id: 0,
            task_state: "TODO",
            gate: "G1",
        },
        Mutation::SetBlock {
            id: 1,
            task_state: "DONE",
            gate: "G1",
        },
        Mutation::AddRequires {
            block: 0,
            required: 1,
        },
        Mutation::AddTag {
            block: 0,
            tag: "agent",
        },
    ];
    let (mv, fresh) = drive(ANTIJOIN, &muts).await;
    assert!(
        !fresh.is_empty(),
        "seed must yield a non-empty readiness set; got {fresh:?}"
    );
    assert_eq!(mv, fresh, "Now.org matview must equal fresh recompute");
}
