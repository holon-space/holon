//! Pin for arm (b) of the unified journals-phantom bug: org-writeback must
//! reconcile a block OFF a document's disk file when the block's AUTHORITATIVE
//! document (per `block_raw` routing) is a DIFFERENT document.
//!
//! Shape (isolated from the journals-seed authority fork, BugFunnel row 25, so
//! this pin goes red for exactly ONE reason — cross-doc membership):
//!   * `DayPage.org` (`#+ID: area-daypage`, a Page) authoritatively OWNS the
//!     child block `bulk-0-0` ("Q1W  q9").
//!   * `Overview.org` (`#+ID: overview`, a Page) is an aggregator that ALSO
//!     carries `bulk-0-0` flat on disk — a STALE copy left by a past mis-route
//!     / crash / external edit (the on-disk poison bf071003 cannot remove).
//!
//! On the current tree the `Overview.org` ingest ADOPTS the stale copy
//! (`find_foreign_blocks` matview attribution → re-parent into
//! `block:overview`), so (b) the block's parent flips to `overview` and (a) the
//! stale copy is never pruned from disk — re-ingest re-adopts it and the org
//! fixed-point oscillates.
//!
//! The fix routes the membership decision through AUTHORITATIVE `block_raw`
//! (`get_block_authoritative`, the bf071003 pattern): a block whose
//! authoritative doc differs from the file being ingested is never adopted; it
//! is pruned from the ingesting file's own honest re-render.
//!
//! @pbt kind harness
//! @pbt covers cross-doc-membership(writeback) — a stale cross-doc block is
//! pruned off an aggregator file and never adopted away from its authoritative
//! page (journals phantom arm b) @pbt overlaps general_e2e_composed_pbt — kept:
//! the keystone has no on-disk-phantom / cross-file-membership transition

use std::sync::Arc;
use std::time::Duration;

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

const STALE_ID: &str = "block:bulk-0-0";
const DAY_PAGE: &str = "block:area-daypage";

const DAY_PAGE_ORG: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

// The aggregator carrying a STALE flat copy of the day-page's child.
const OVERVIEW_ORG: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

async fn read_file(env: &TestEnvironment, name: &str) -> String {
    use holon_filesystem::FileSystem;
    let path = env.org_root().join(name);
    env.org_fs.read_to_string(&path).await.unwrap_or_default()
}

async fn parent_of(env: &TestEnvironment, id: &str) -> Option<String> {
    let rows = env
        .query_sql(&format!(
            "SELECT parent_id FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .expect("query block_raw parent_id");
    rows.first()
        .and_then(|r| r.get("parent_id").and_then(|v| v.as_string()))
        .map(|s| s.to_string())
}

async fn count_rows(env: &TestEnvironment, id: &str) -> usize {
    env.query_sql(&format!("SELECT id FROM block_raw WHERE id = '{id}'"))
        .await
        .expect("query block_raw count")
        .len()
}

#[test]
fn stale_cross_doc_block_is_pruned_not_adopted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        // Desktop dogfood shape: SqlOnly (Loro off).
        env.set_enable_loro(false);

        // Establish the authoritative owner first: DayPage.org OWNS bulk-0-0.
        env.write_org_file("DayPage.org", DAY_PAGE_ORG)
            .await
            .expect("write DayPage.org");

        env.start_app(true).await.expect("start_app");

        // Wait for bulk-0-0 to land under the day-page.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if count_rows(&env, STALE_ID).await >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "bulk-0-0 never landed from DayPage.org"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "precondition: bulk-0-0 must be owned by the day-page before the phantom appears"
        );
        tokio::time::sleep(Duration::from_millis(400)).await;

        // Inject the on-disk phantom: Overview.org gains a STALE flat copy.
        let path = env
            .write_org_file("Overview.org", OVERVIEW_ORG)
            .await
            .expect("write Overview.org");
        let seq = env.org_fs.last_change_seq();
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        // Let writeback settle (prune + any re-render).
        tokio::time::sleep(Duration::from_millis(800)).await;

        let overview_disk = read_file(&env, "Overview.org").await;
        let day_disk = read_file(&env, "DayPage.org").await;

        // (b) exactly once, still owned by the day-page — NOT re-parented to overview.
        assert_eq!(
            count_rows(&env, STALE_ID).await,
            1,
            "bulk-0-0 must exist exactly once. day={day_disk}\noverview={overview_disk}"
        );
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "bulk-0-0 was ADOPTED away from its authoritative day-page (parent flipped to \
             overview) — the stale cross-doc copy was silently adopted instead of pruned. \
             overview.org=\n{overview_disk}"
        );

        // (a) the stale copy is pruned off the aggregator's disk file.
        assert!(
            !overview_disk.contains("bulk-0-0"),
            "Overview.org still carries the stale bulk-0-0 copy — it was not pruned:\n{overview_disk}"
        );

        // (c) fixed-point: a second settle changes neither file (no oscillation).
        let _ = path;
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(
            read_file(&env, "Overview.org").await,
            overview_disk,
            "Overview.org kept changing after settle — org fixed-point oscillates"
        );
        assert_eq!(
            read_file(&env, "DayPage.org").await,
            day_disk,
            "DayPage.org kept changing after settle — org fixed-point oscillates"
        );

        env.stop_app().await.expect("stop_app");
    });
}
