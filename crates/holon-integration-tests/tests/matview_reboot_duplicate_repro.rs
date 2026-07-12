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

use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironment;

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
    });
}
