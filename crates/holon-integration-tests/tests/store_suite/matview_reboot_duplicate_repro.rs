//! Reproducer for BugFunnel row 90 (dogfood #4, 2026-07-12):
//! "After app restart over an existing DB, the `block` IVM matview returns
//! 2-3 DUPLICATE rows per id (block_raw is clean)".
//!
//! Class: Turso-IVM consolidation on the reboot/re-ingest path. The `block`
//! matview's backing btree AND its persisted DBSP state survive a process
//! restart (the SELECT is unchanged, so `reconcile_named_view` is a no-op and
//! never DROP+CREATEs it). On the second boot the org/Loro re-ingest rewrites
//! `block_raw` (the `projection_hash` consolidator tag forces a full
//! re-ingest), feeding deltas into the persisted matview. If those deltas are
//! not consolidated against the surviving state, each block ends up with
//! multiple identical matview rows while `block_raw` stays unique (PK dedup).
//!
//! This test boots the full composed stack over a temp DB, seeds blocks,
//! shuts down cleanly, boots AGAIN over the SAME DB dir, and asserts:
//!   SELECT id, COUNT(*) FROM block GROUP BY id HAVING COUNT(*) > 1  is EMPTY,
//! and per-id the `block` matview count matches `block_raw`.
//!
//! Expected to FAIL until the reboot consolidation gap is fixed.
//!
//! @pbt kind harness
//! @pbt covers reboot-matview-dup — duplicate matview rows after reboot
//! (BugFunnel 90) @pbt overlaps general_e2e_composed_pbt — kept: no reboot
//! transition in keystone

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
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

const VAULT_ORG: &str = "\
* Alpha
:PROPERTIES:
:ID: blk-a
:END:
* Beta
:PROPERTIES:
:ID: blk-b
:END:
* Gamma
:PROPERTIES:
:ID: blk-c
:END:
";

/// Wait until the org scan has landed the three seeded blocks in `block_raw`.
async fn wait_for_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(
                "SELECT id FROM block_raw WHERE id IN ('block:blk-a', 'block:blk-b', \
                 'block:blk-c')",
            )
            .await
            .expect("query block_raw");
        if rows.len() >= 3 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated all three blocks (have {} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `(id, count)` for every `id` appearing more than once in the `block`
/// matview.
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

/// Every `id` whose `block` matview row-count differs from its `block_raw`
/// row-count. `block_raw` is PK-unique (exactly 1 per id), so any id with a
/// matview count != 1 is a reboot-consolidation defect (row 91's exact
/// invariant: "matview row-count == base row-count per id").
async fn matview_base_mismatches(env: &TestEnvironment) -> Vec<(String, i64, i64)> {
    let rows = env
        .query(
            // The `block` matview intentionally filters the parent sentinel
            // (`WHERE b.id != 'sentinel:no_parent'`), so exclude it here too;
            // every other id must appear exactly once in both.
            "SELECT br.id AS id, (SELECT COUNT(*) FROM block_raw r WHERE r.id = br.id) AS \
             base_cnt, (SELECT COUNT(*) FROM block m WHERE m.id = br.id) AS mv_cnt FROM block_raw \
             br WHERE br.id != 'sentinel:no_parent' GROUP BY br.id HAVING base_cnt != mv_cnt",
            QueryLanguage::HolonSql,
        )
        .await
        .expect("reconciliation query");
    rows.iter()
        .map(|r| {
            let id = r
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("<none>")
                .to_string();
            let base = r.get("base_cnt").and_then(|v| v.as_i64()).unwrap_or(-1);
            let mv = r.get("mv_cnt").and_then(|v| v.as_i64()).unwrap_or(-1);
            (id, base, mv)
        })
        .collect()
}

/// Every `(block, edge field)` whose hydrated array in the `block` matview
/// differs, AS A MULTISET, from the base junction's targets for that block.
///
/// The row-count assertions above cannot see this class: a corrupt target list
/// lives INSIDE a single matview row, so `block` still has exactly one row per
/// id and every count matches while the served array reads
/// `["block:x","block:x"]`. That is the shape Martin's vault came back in
/// (bugfunnel `2026-08-30-matview-edge-agg-doubles-every-requires-target`):
/// the per-junction agg matview doubled every `requires` target while
/// `block_requires` stayed PK-clean, and org write-back wrote the doubled
/// `:REQUIRES:` drawer to disk.
///
/// Both sides are expanded to their elements and SORTED before comparison, so
/// this is multiset equality: it catches a doubled target (`[x] -> [x,x]`), a
/// dropped one, AND a same-length substitution (`[x,y] -> [x,x]`) that a
/// cardinality check would pass. Order is not semantic for an edge set — the
/// junction hydration has no `ORDER BY` — so sorting is what makes the
/// comparison meaningful rather than flaky.
///
/// MATVIEW-ANCHORED, deliberately: the scan is `FROM block m` with the junction
/// read as a correlated subquery, so a junction row whose block is missing from
/// the `block` matview is not examined here. That direction is already covered
/// one assertion earlier — `matview_base_mismatches` walks `block_raw` and
/// fails when a block's matview row-count differs from its base row-count, so a
/// block dropped from the matview is caught there rather than silently. Making
/// this query bidirectional would duplicate that check, not add reach.
///
/// That hand-off DEPENDS on every junction's `ON DELETE CASCADE` FK to
/// `block_raw` (block_requires.sql and siblings): it is what keeps a junction
/// row from outliving its block, so "orphan junction rows" reduces to "a block
/// the matview lost", which the row-count check sees. Load-bearing rather than
/// incidental — the fork's deferred-FK/autocommit wart means FK enforcement is
/// not something to assume — so if those cascades are ever relaxed, this
/// comparison must gain the reverse direction.
///
/// Iterates `EdgeField::ALL` so a fifth edge field cannot be half-covered.
async fn edge_array_multiset_mismatches(env: &TestEnvironment) -> Vec<String> {
    let mut out = Vec::new();
    for field in holon_api::EdgeField::ALL {
        let (junction, source_col, target_col) = match field {
            holon_api::EdgeField::Tags => ("block_tags", "block_id", "tag"),
            holon_api::EdgeField::Requires => ("block_requires", "block_id", "required_id"),
            holon_api::EdgeField::AdviceSuppressed => {
                ("advice_suppressed", "anchor_id", "lesson_id")
            }
            holon_api::EdgeField::ContributesTo => {
                ("block_contributes_to", "block_id", "target_id")
            }
        };
        let column = field.column();
        // `IS NOT` (not `!=`) so a block with no targets on either side — where
        // both group_concat aggregates are NULL — compares equal.
        //
        // Joined on US (0x1f), not the default comma: tags are free-form
        // strings, so a comma separator makes `{"a,b","c"}` and `{"a","b,c"}`
        // concat to the same text and any corruption that splits or merges a
        // target across a comma becomes undetectable.
        let rows = env
            .query(
                &format!(
                    "SELECT id, mv, base FROM (SELECT m.id AS id, (SELECT group_concat(value, \
                     char(31)) FROM (SELECT value FROM json_each(m.{column}) ORDER BY value)) AS \
                     mv, (SELECT group_concat(t, char(31)) FROM (SELECT j.{target_col} AS t FROM \
                     {junction} j WHERE j.{source_col} = m.id ORDER BY t)) AS base FROM block m) \
                     WHERE mv IS NOT base"
                ),
                QueryLanguage::HolonSql,
            )
            .await
            .unwrap_or_else(|e| panic!("edge-array multiset query for `{column}`: {e:#}"));
        for r in &rows {
            let cell = |k: &str| {
                r.get(k)
                    .and_then(|v| v.as_string())
                    .unwrap_or("<empty>")
                    .to_string()
            };
            out.push(format!(
                "{}.{column}: matview array = [{}], junction = [{}]",
                cell("id"),
                cell("mv"),
                cell("base")
            ));
        }
    }
    out
}

/// A `LoroBackend` over the frontend's authority doc — the production
/// edge-field write path (Loro -> project() -> SQL), mirroring
/// `matview_duplicate_row_repro`.
async fn edge_backend(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_doc(DocScope::Global)
        .await
        .expect("global Loro doc");
    LoroBackend::from_document(global_doc)
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Full-matview vs base-table row-count reconciliation, for diagnostics.
async fn dump_counts(env: &TestEnvironment) -> String {
    let raw = env
        .query_sql("SELECT COUNT(*) AS c FROM block_raw")
        .await
        .expect("count block_raw");
    let mv = env
        .query_sql("SELECT COUNT(*) AS c FROM block")
        .await
        .expect("count block");
    let raw_c = raw
        .first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_i64());
    let mv_c = mv.first().and_then(|r| r.get("c")).and_then(|v| v.as_i64());
    format!("block_raw rows={raw_c:?}  block matview rows={mv_c:?}")
}

#[test]
fn block_matview_no_duplicates_after_reboot_over_existing_db() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");

        // ── Boot 1 ──────────────────────────────────────────────────────
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;
        eprintln!("[reboot-dup] boot-1 {}", dump_counts(&env).await);
        let dupes_boot1 = duplicate_ids(&env).await;
        assert!(
            dupes_boot1.is_empty(),
            "boot-1 already has duplicates (unexpected): {dupes_boot1:?}"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        eprintln!("[reboot-dup] boot-2 {}", dump_counts(&env).await);

        let dupes = duplicate_ids(&env).await;
        assert!(
            dupes.is_empty(),
            "[reboot] `block` matview has DUPLICATE rows after restart over an existing DB: \
             {dupes:?}\n{}",
            dump_counts(&env).await
        );
        let mismatches = matview_base_mismatches(&env).await;
        assert!(
            mismatches.is_empty(),
            "[reboot] `block` matview row-count diverges from `block_raw` per id (id, base, \
             matview): {mismatches:?}\n{}",
            dump_counts(&env).await
        );
    });
}

/// Row-91 faithful variant: the duplicate in the live dogfood carried EDGE
/// FIELDS (tags {proj} + requires). The reboot re-ingest re-asserts those
/// junction rows, feeding deltas into the SHARED per-junction agg matviews
/// (`block_tags_agg` / `block_requires_agg`) whose output the persisted `block`
/// JOIN matview consumes. This is the exact tag/requires re-assert path row 150
/// identified as doubling the matview row after the reopen-triggered
/// autocheckpoint. Asserts BOTH no-duplicate-id AND per-id matview==base.
#[test]
fn block_matview_with_edge_fields_no_duplicates_after_reboot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");

        // ── Boot 1: seed, then write tags + requires through the prod edge path
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        backend
            .set_block_tags("block:blk-a", &["proj".to_string()])
            .await
            .expect("set_block_tags(blk-a)");
        // ALLOW(entity_uri_from_raw): test-caller-supplied id; from_raw schemes it.
        backend
            .set_block_requires("block:blk-a", &[EntityUri::from_raw("block:blk-b")])
            .await
            .expect("set_block_requires(blk-a)");
        settle(&env).await;

        eprintln!("[reboot-edge] boot-1 {}", dump_counts(&env).await);
        assert!(
            duplicate_ids(&env).await.is_empty(),
            "boot-1 with edge fields already has duplicates (unexpected)"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;
        eprintln!("[reboot-edge] boot-2 {}", dump_counts(&env).await);

        let dupes = duplicate_ids(&env).await;
        assert!(
            dupes.is_empty(),
            "[reboot-edge] `block` matview has DUPLICATE rows after restart with edge fields \
             present: {dupes:?}\n{}",
            dump_counts(&env).await
        );
        let mismatches = matview_base_mismatches(&env).await;
        assert!(
            mismatches.is_empty(),
            "[reboot-edge] `block` matview row-count diverges from `block_raw` per id (id, base, \
             matview): {mismatches:?}\n{}",
            dump_counts(&env).await
        );

        // The row counts above all match even when an edge array is corrupt
        // INSIDE its row, so compare the arrays themselves as multisets.
        let edge_mismatches = edge_array_multiset_mismatches(&env).await;
        assert!(
            edge_mismatches.is_empty(),
            "[reboot-edge] a hydrated edge array in the `block` matview differs as a multiset \
             from its base junction after the restart — the vault-doubling class: \
             {edge_mismatches:?}\n{}",
            dump_counts(&env).await
        );
    });
}
