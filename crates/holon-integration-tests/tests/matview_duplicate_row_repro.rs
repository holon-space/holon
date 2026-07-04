//! Reproducer for the "duplicate row in the `block` matview" bug candidate
//! surfaced by the composed keystone PBT at forced weights.
//!
//! Shape (per the PBT shrink): on a fresh composed app with two existing text
//! blocks, apply to ONE of them, in order,
//!   (1) an edge-field `tags` write setting tags {"proj"}, then
//!   (2) an edge-field `requires` write setting requires [the other block],
//! and the SUT's `block` MATERIALIZED VIEW ends up with TWO identical rows for
//! that block (same id, tags, requires — everything identical). The reference
//! (base) state is correct (one row). No advice involved.
//!
//! The writes route through the SAME production edge-field writers the PBT uses
//! (`LoroBackend::set_block_tags` / `set_block_requires` over the frontend's
//! authority doc — see `pbt::op_write_cap::EdgeFieldWriter`), so the write flows
//! Loro → `project()` → SQL exactly as production does.
//!
//! The `block` matview (synthesized by `BlockMatviewSchemaModule`, see
//! crates/holon-turso/src/schema_modules.rs) previously GROUP BYed over the
//! LEFT-JOIN cross-product of THREE junctions in one view — plain-SQL fan-out
//! semantics (see `matview_fanout_three_tags_then_one_requires` below). It is
//! now chained: one agg matview per junction, `block` LEFT JOINs the aggs.
//!
//! This test boots the minimal composed Loro+Turso stack (no UI), applies the
//! two edge writes, settles projection, and asserts the `block` matview has no
//! duplicate `id`. It is expected to FAIL, demonstrating the duplicate.

use std::sync::Arc;
use std::time::Duration;

use holon_api::{EntityUri, QueryLanguage};
use holon_integration_tests::TestEnvironment;
use holon_loro::LoroBackend;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// Two headed blocks with explicit IDs so the write targets are deterministic.
const VAULT_ORG: &str = "\
* Alpha
:PROPERTIES:
:ID: blk-a
:END:
* Beta
:PROPERTIES:
:ID: blk-b
:END:
";

const BLK_A: &str = "block:blk-a";
const BLK_B: &str = "block:blk-b";

/// Boot a composed Loro+Turso env with the two blocks loaded into both stores.
async fn boot_two_blocks(runtime: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
    assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
    env.write_org_file("vault.org", VAULT_ORG)
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("start_app");

    // Wait until the org scan landed both blocks in SQL (the `block_raw`
    // structural rows the matview hydrates from).
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw WHERE id IN ('block:blk-a', 'block:blk-b')")
            .await
            .expect("query block_raw");
        if rows.len() >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated both blocks (have {} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    env
}

/// A `LoroBackend` over the frontend's authority doc — the exact construction
/// the production `EdgeFieldWriter` (and `LoroBlockOperations::get_backend`) use,
/// so the edge writes flow Loro → project() → SQL as in prod.
async fn edge_backend(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_global_doc()
        .await
        .expect("global Loro doc");
    LoroBackend::from_document(global_doc)
}

/// Settle the Loro→SQL projection: wait for the sync controller to drain the
/// commit into SQL, then for the matview CDC to go quiescent.
async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    // Small extra floor so the last CDC batch fully lands in the matview.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

async fn set_tags(env: &TestEnvironment, block: &str, tags: &[&str]) {
    let backend = edge_backend(env).await;
    let tags: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    backend
        .set_block_tags(block, &tags)
        .await
        .unwrap_or_else(|e| panic!("set_block_tags({block}) failed: {e:#}"));
    settle(env).await;
}

async fn set_requires(env: &TestEnvironment, block: &str, targets: &[&str]) {
    let backend = edge_backend(env).await;
    let reqs: Vec<EntityUri> = targets
        // ALLOW(entity_uri_from_raw): test-caller-supplied id; from_raw normalizes to schemed form
        .iter()
        .map(|s| EntityUri::from_raw(s))
        .collect();
    backend
        .set_block_requires(block, &reqs)
        .await
        .unwrap_or_else(|e| panic!("set_block_requires({block}) failed: {e:#}"));
    settle(env).await;
}

/// Return `(id, count)` for every `id` that appears more than once in the
/// `block` matview.
async fn duplicate_ids(env: &TestEnvironment) -> Vec<(String, i64)> {
    let rows = env
        .query(
            "SELECT id, COUNT(*) AS cnt FROM block GROUP BY id HAVING COUNT(*) > 1",
            QueryLanguage::HolonSql,
        )
        .await
        .expect("dup-count query");
    rows.iter()
        .map(|r| {
            let id = r
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("<none>")
                .to_string();
            let cnt = r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(-1);
            (id, cnt)
        })
        .collect()
}

/// Dump the full matview rows for one id (for failure diagnostics).
async fn dump_rows(env: &TestEnvironment, block: &str) -> String {
    let rows = env
        .query(
            &format!(
                "SELECT id, tags, requires, advice_suppressed FROM block WHERE id = '{block}'"
            ),
            QueryLanguage::HolonSql,
        )
        .await
        .expect("dump query");
    let mut out = format!("{} matview row(s) for {block}:\n", rows.len());
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!(
            "  [{i}] tags={:?} requires={:?} advice_suppressed={:?}\n",
            r.get("tags").and_then(|v| v.as_string()),
            r.get("requires").and_then(|v| v.as_string()),
            r.get("advice_suppressed").and_then(|v| v.as_string()),
        ));
    }
    out
}

async fn assert_no_dupes(env: &TestEnvironment, scenario: &str) {
    let dupes = duplicate_ids(env).await;
    if !dupes.is_empty() {
        let dump = dump_rows(env, BLK_A).await;
        panic!("[{scenario}] `block` matview has DUPLICATE rows: {dupes:?}\n{dump}");
    }
}

/// PRIMARY REPRO: tags {proj} then requires [blk-b] on blk-a.
#[test]
fn matview_duplicate_after_tags_then_requires() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["proj"]).await;
        eprintln!(
            "[tags_then_requires] after tags:\n{}",
            dump_rows(&env, BLK_A).await
        );
        set_requires(&env, BLK_A, &[BLK_B]).await;
        eprintln!(
            "[tags_then_requires] after requires:\n{}",
            dump_rows(&env, BLK_A).await
        );
        assert_no_dupes(&env, "tags_then_requires").await;
    });
}

/// NARROW 1: reverse order — requires then tags.
#[test]
fn matview_duplicate_after_requires_then_tags() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_requires(&env, BLK_A, &[BLK_B]).await;
        set_tags(&env, BLK_A, &["proj"]).await;
        eprintln!(
            "[requires_then_tags] final:\n{}",
            dump_rows(&env, BLK_A).await
        );
        assert_no_dupes(&env, "requires_then_tags").await;
    });
}

/// NARROW 2: tags-only, written twice (both junctions never both touched).
#[test]
fn matview_duplicate_after_tags_twice() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["proj"]).await;
        set_tags(&env, BLK_A, &["proj", "other"]).await;
        eprintln!("[tags_twice] final:\n{}", dump_rows(&env, BLK_A).await);
        assert_no_dupes(&env, "tags_twice").await;
    });
}

/// Parse a JSON-array column of blk-a's (single) `block` matview row.
async fn parsed_edge_col(env: &TestEnvironment, col: &str) -> Vec<String> {
    let rows = env
        .query(
            &format!("SELECT {col} FROM block WHERE id = '{BLK_A}'"),
            QueryLanguage::HolonSql,
        )
        .await
        .expect("edge-col query");
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one `block` row for {BLK_A}"
    );
    let raw = rows[0]
        .get(col)
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("{col} column missing/non-text"))
        .to_string();
    let mut parsed: Vec<String> = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{col} is not a JSON string array ({raw}): {e}"));
    parsed.sort();
    parsed
}

/// FAN-OUT REPRO: 3 tags then 1 requires on blk-a. The pre-fix single-view
/// GROUP BY over the LEFT-JOIN cross-product of the three junctions repeats
/// the single requires value once per tag (`["blk-b","blk-b","blk-b"]`).
/// The `block` matview row must carry exactly 3 tags and exactly 1 requires.
#[test]
fn matview_fanout_three_tags_then_one_requires() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["t1", "t2", "t3"]).await;
        set_requires(&env, BLK_A, &[BLK_B]).await;
        eprintln!("[fanout] final:\n{}", dump_rows(&env, BLK_A).await);

        assert_eq!(
            parsed_edge_col(&env, "tags").await,
            vec!["t1", "t2", "t3"],
            "tags must have exactly the 3 written values, no duplicates"
        );
        assert_eq!(
            parsed_edge_col(&env, "requires").await,
            vec![BLK_B.to_string()],
            "requires must have exactly ONE element (fan-out repeats it per tag)"
        );
        assert_eq!(
            parsed_edge_col(&env, "advice_suppressed").await,
            Vec::<String>::new(),
            "advice_suppressed was never written"
        );
        assert_no_dupes(&env, "fanout").await;
    });
}

/// PAGE-TAG shapes (docblock-count divergence, 2026-07-07). NEGATIVE RESULTS:
/// these single-block shapes do NOT reproduce the duplicate (they PASS). The
/// real keystone divergence needs cross-block INTERLEAVED deltas through the
/// SHARED per-junction agg matviews under PER-STATEMENT (autocommit) IVM
/// maintenance — reproduced Turso-side by
/// `turso/tests/integration/query_processing/test_ivm_block_matview_replay.rs`
/// (`REPLAY_MODE=auto`, `#[ignore]`d, seed-flaky). Kept here to document that
/// the naive group-empties/refill shape alone is insufficient.
#[test]
fn matview_duplicate_after_tag_then_delete_all_tags() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["Page"]).await;
        eprintln!(
            "[tag_then_delete] after tags {{Page}}:\n{}",
            dump_rows(&env, BLK_A).await
        );
        set_tags(&env, BLK_A, &[]).await;
        eprintln!(
            "[tag_then_delete] after tags {{}}:\n{}",
            dump_rows(&env, BLK_A).await
        );
        assert_no_dupes(&env, "tag_then_delete").await;
    });
}

/// PAGE-TAG REPRO variant: two tags {Page, x} then REPLACE with {} (delete both).
#[test]
fn matview_duplicate_after_two_tags_then_delete_all() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["Page", "x"]).await;
        eprintln!(
            "[two_tags_then_delete] after tags {{Page,x}}:\n{}",
            dump_rows(&env, BLK_A).await
        );
        set_tags(&env, BLK_A, &[]).await;
        eprintln!(
            "[two_tags_then_delete] after tags {{}}:\n{}",
            dump_rows(&env, BLK_A).await
        );
        assert_no_dupes(&env, "two_tags_then_delete").await;
    });
}

/// PAGE-TAG REPRO variant matching the exact keystone SQL trigger: tags {Page}
/// then a SINGLE write that BOTH clears tags and clears requires (the
/// `INSERT block_tags Page; DELETE block_tags; DELETE block_requires` stream).
#[test]
fn matview_duplicate_after_tag_then_clear_tags_and_requires() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_tags(&env, BLK_A, &["Page"]).await;
        set_requires(&env, BLK_A, &[BLK_B]).await;
        eprintln!(
            "[tag_then_clear_both] after tags+requires:\n{}",
            dump_rows(&env, BLK_A).await
        );
        set_tags(&env, BLK_A, &[]).await;
        set_requires(&env, BLK_A, &[]).await;
        eprintln!(
            "[tag_then_clear_both] after clear both:\n{}",
            dump_rows(&env, BLK_A).await
        );
        assert_no_dupes(&env, "tag_then_clear_both").await;
    });
}

/// NARROW 3: requires-only, written twice.
#[test]
fn matview_duplicate_after_requires_twice() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot_two_blocks(rt).await;
        set_requires(&env, BLK_A, &[BLK_B]).await;
        set_requires(&env, BLK_A, &[BLK_B]).await;
        eprintln!("[requires_twice] final:\n{}", dump_rows(&env, BLK_A).await);
        assert_no_dupes(&env, "requires_twice").await;
    });
}
