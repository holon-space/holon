//! Scale-tier ingest test: ONE org document with thousands of blocks.
//!
//! Martin's real vault holds a single 2.1 MB `Projects/Holon.org` with 15,763
//! `:ID:`-bearing headlines nested 8 levels deep and ZERO links. Ingesting it
//! wedges the app permanently: the Turso actor pins a core at 100% and every
//! subsequent query — UI, MCP, the remaining files' ingest — hangs forever
//! behind the single actor. This test reproduces that shape headlessly.
//!
//! The keystone PBT cannot see this: its generators build documents of tens of
//! blocks, so no case ever approaches the scale where the defect appears.
//!
//! Block count is env-gated so a normal sweep pays milliseconds:
//!   HOLON_SCALE_BLOCKS=15000 cargo nextest run -p holon-integration-tests \
//!     single_large_document --no-capture
//!
//! @pbt kind harness
//! @pbt covers single-document-scale — one document with N blocks stays live

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_integration_tests::TestEnvironmentBuilder;

/// Depth ladder distilled from the real file's headline-level histogram
/// (levels 4-6 dominate, tail to 8). Every step rises by at most one level, so
/// the emitted outline is valid org at any starting offset.
const LEVEL_LADDER: &[usize] = &[
    2, 3, 4, 5, 6, 6, 6, 7, 8, 6, 5, 6, 6, 7, 6, 5, 6, 6, 6, 6, 4, 5, 6, 6, 7, 8, 6, 5, 6, 6,
];

fn blocks() -> usize {
    match std::env::var("HOLON_SCALE_BLOCKS") {
        Ok(raw) => raw
            .parse()
            .unwrap_or_else(|e| panic!("HOLON_SCALE_BLOCKS='{raw}' is not a block count: {e}")),
        Err(std::env::VarError::NotPresent) => 200,
        Err(e) => panic!("HOLON_SCALE_BLOCKS unreadable: {e}"),
    }
}

/// One org document with `n` `:ID:`-bearing headlines in a deep outline.
fn synthetic_document(n: usize) -> String {
    let mut out = String::with_capacity(n * 140);
    out.push_str("* Scale Root\n:PROPERTIES:\n:ID: 5ca1e000-0000-4000-8000-000000000000\n:END:\n");
    for i in 0..n {
        let level = LEVEL_LADDER[i % LEVEL_LADDER.len()];
        out.push_str(&"*".repeat(level));
        out.push_str(&format!(
            " Node {i}\n:PROPERTIES:\n:ID: 5ca1e000-0000-4000-8000-{n:012x}\n:END:\n",
            n = i + 1
        ));
    }
    out
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// A trivial query through the Turso actor. Hangs iff the actor is wedged.
async fn count_blocks(env: &holon_integration_tests::TestEnvironment) -> usize {
    let rows = env
        .engine()
        .execute_query("SELECT id FROM block_raw".to_string(), HashMap::new(), None)
        .await
        .expect("query block_raw");
    rows.len()
}

/// The exact `CacheBlockReader::get_blocks` recursive CTE (crates/holon-app/
/// src/turso_seams.rs). Timed here to expose its growth law in N.
const GET_BLOCKS_CTE: &str = "WITH RECURSIVE descendants(id, depth_acc) AS ( SELECT b.id, 0 FROM \
                              block_raw b LEFT JOIN block_tags bt ON bt.block_id = b.id AND \
                              bt.tag = 'Page' WHERE b.parent_id = $doc_id AND bt.block_id IS NULL \
                              UNION ALL SELECT b.id, d.depth_acc + 1 FROM block_raw b JOIN \
                              descendants d ON b.parent_id = d.id LEFT JOIN block_tags bt ON \
                              bt.block_id = b.id AND bt.tag = 'Page' WHERE bt.block_id IS NULL \
                              AND d.depth_acc < 100 ) SELECT b.id FROM block_raw b JOIN \
                              descendants d ON d.id = b.id ORDER BY b.sort_key, b.id";

/// Same result set, but every predicate is a correlated equality the planner
/// is known to index (proved by the `[scale idx]` probes), and the final
/// `JOIN block_raw` is gone. Isolates how much of the quadratic cost is the
/// rewritable joins versus the recursive arm's `b.parent_id = d.id`.
const GET_BLOCKS_CTE_NOT_EXISTS: &str = "WITH RECURSIVE descendants(id, depth_acc) AS ( SELECT b.id, 0 FROM block_raw b WHERE \
     b.parent_id = $doc_id AND NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id \
     AND bt.tag = 'Page') UNION ALL SELECT b.id, d.depth_acc + 1 FROM block_raw b JOIN \
     descendants d ON b.parent_id = d.id WHERE NOT EXISTS (SELECT 1 FROM block_tags bt WHERE \
     bt.block_id = b.id AND bt.tag = 'Page') AND d.depth_acc < 100 ) SELECT id FROM descendants";

/// The pre-Phase-5 shape: one flat scan of `block_raw`, tree walk in Rust.
/// O(N) in SQL — the lower bound any document read can hope for here.
const FLAT_DOC_SCAN: &str = "SELECT b.id, b.parent_id FROM block_raw b";

#[test]
fn single_large_document_leaves_the_actor_responsive() {
    let n = blocks();
    // The +1 is the root headline the ladder hangs off.
    let expected = n + 1;
    // Generous: 40 ms/block covers a cold debug-profile machine. Anything that
    // needs more than this is the wedge, not slowness.
    let budget = Duration::from_millis((expected as u64 * 40).max(60_000));

    // This suite's oracle is WALL-CLOCK ingest budget, and the shadow issues
    // authority reads on the same Turso actor — measurement apparatus competing
    // with the thing being measured (design doc §9.8, same principle as the
    // read-budget suites). Must precede the TestEnvironmentBuilder boot.
    holon_orgmode::writeback_shadow::disable_for_budget_suite();

    // Installs the RUST_LOG-driven subscriber, so HOLON_ACTOR_STATS reports
    // reach the test output.
    holon_integration_tests::test_tracing::SpanCollector::global();

    let rt = runtime();
    rt.clone().block_on(async move {
        let doc = synthetic_document(n);
        let t0 = Instant::now();

        let env = tokio::time::timeout(
            budget,
            TestEnvironmentBuilder::new()
                .with_org_file("Projects/Scale.org", doc)
                .build(rt.clone()),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "boot never completed for a {expected}-block document within {:?} — the Turso \
                 actor is wedged during/after ingest",
                budget
            )
        })
        .expect("boot scale vault");

        // Ingest is asynchronous: poll until every block has landed. Each poll
        // is a round trip through the single Turso actor, so a wedged actor
        // shows up here as a timeout rather than a wrong count.
        let deadline = Instant::now() + budget;
        let mut seen = 0usize;
        while Instant::now() < deadline {
            seen = tokio::time::timeout(Duration::from_secs(30), count_blocks(&env))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "a plain `SELECT id FROM block_raw` did not return within 30s after \
                         ingesting {expected} blocks — the Turso actor is wedged (single actor: \
                         one pathological query takes the whole process hostage)"
                    )
                });
            if seen >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        let elapsed = t0.elapsed();
        assert!(
            seen >= expected,
            "only {seen}/{expected} blocks ingested from one document within {budget:?} \
             (elapsed {elapsed:?})"
        );

        // The actor must still serve queries AFTER the batch commits — that is
        // where the wedge lands in production.
        for probe in 0..5 {
            tokio::time::timeout(Duration::from_secs(30), count_blocks(&env))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "post-ingest probe {probe} hung >30s on `SELECT id FROM block_raw` — the \
                         actor is wedged after committing {expected} blocks"
                    )
                });
        }

        eprintln!(
            "[scale] {expected} blocks in one document: ingest+settle {elapsed:?} \
             ({:.2} ms/block)",
            elapsed.as_secs_f64() * 1000.0 / expected as f64
        );

        // The document-read CTE in isolation: this is the query the actor
        // spends its life in once a document gets large.
        let doc_rows = env
            .engine()
            .execute_query(
                "SELECT id, parent_id FROM block_raw WHERE content = 'Scale Root'".to_string(),
                HashMap::new(),
                None,
            )
            .await
            .expect("find doc ids");
        let doc_id = doc_rows
            .first()
            .and_then(|r| r.get("parent_id"))
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| panic!("no 'Scale Root' block in block_raw: {doc_rows:?}"))
            .to_string();

        let mut params = HashMap::new();
        params.insert(
            "doc_id".to_string(),
            holon_api::Value::String(doc_id.clone()),
        );
        let plan = env
            .engine()
            .execute_query(
                format!("EXPLAIN QUERY PLAN {GET_BLOCKS_CTE}"),
                params.clone(),
                None,
            )
            .await
            .expect("explain get_blocks CTE");
        for row in &plan {
            eprintln!("[scale plan] {row:?}");
        }

        for probe_sql in [
            "SELECT id FROM block_raw WHERE parent_id = $doc_id",
            "SELECT tag FROM block_tags WHERE block_id = $doc_id",
            "SELECT id FROM block_raw WHERE id = $doc_id",
        ] {
            let p = env
                .engine()
                .execute_query(
                    format!("EXPLAIN QUERY PLAN {probe_sql}"),
                    params.clone(),
                    None,
                )
                .await
                .expect("explain probe");
            let detail: Vec<String> = p
                .iter()
                .filter_map(|r| r.get("detail").and_then(|v| v.as_string()))
                .map(|s| s.to_string())
                .collect();
            eprintln!("[scale idx] {probe_sql} => {detail:?}");
        }

        // The comparison ladder: production shape vs. two alternatives with the
        // same result set. Prints the growth law so a fix can be judged on
        // complexity class, not on one data point.
        let mut prod_ms = 0.0f64;
        for (label, sql) in [
            ("prod", GET_BLOCKS_CTE),
            ("no-exists-join", GET_BLOCKS_CTE_NOT_EXISTS),
            ("flat-scan", FLAT_DOC_SCAN),
        ] {
            let t_cte = Instant::now();
            let rows = env
                .engine()
                .execute_query(sql.to_string(), params.clone(), None)
                .await
                .unwrap_or_else(|e| panic!("run {label} document read: {e}"));
            let took = t_cte.elapsed();
            if label == "prod" {
                prod_ms = took.as_secs_f64() * 1000.0;
            }
            eprintln!("[scale cte] {label} rows={} in {took:?}", rows.len());
            let p = env
                .engine()
                .execute_query(format!("EXPLAIN QUERY PLAN {sql}"), params.clone(), None)
                .await
                .unwrap_or_else(|e| panic!("explain {label}: {e}"));
            let detail: Vec<String> = p
                .iter()
                .filter_map(|r| r.get("detail").and_then(|v| v.as_string()))
                .map(|s| s.to_string())
                .collect();
            eprintln!("[scale cte] {label} plan={detail:?}");
        }

        // A document read is issued on EVERY block change of that document
        // (file_sync_controller → CacheBlockReader::get_blocks), so it must be
        // at most linear in the document's block count. 0.5 ms/block is ~80×
        // slower than the flat scan measured alongside — a ceiling generous
        // enough that only a change of complexity class breaks it.
        let read_budget_ms = (expected as f64 * 0.5).max(500.0);
        assert!(
            prod_ms <= read_budget_ms,
            "the document-read recursive CTE took {prod_ms:.0}ms for {expected} blocks (budget \
             {read_budget_ms:.0}ms). It is QUADRATIC: EXPLAIN shows `SCAN block_raw` for the \
             recursive arm's `b.parent_id = d.id` instead of SEARCH via \
             idx_block_raw_parent_id, so every descendant costs a full table scan. Compare the \
             `flat-scan` line above — same rows, linear. This query is what pins the Turso \
             actor at 100% forever on Martin's 15,763-block Projects/Holon.org."
        );
        let _ = doc_id;
    });
}
