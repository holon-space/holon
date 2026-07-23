//! Regression lock for the 2026-07-23 keystone "org-render link-adoption echo"
//! (job 72446a9c), root-caused to the runtime `convert_block_to_page` leaving
//! its new page FILELESS.
//!
//! `convert_block_to_page` mints a NEW page P under the origin's nearest
//! page-ancestor, RE-HOMES the origin's children under P, and rewrites the
//! origin into a link to P (intended adoption; proven by
//! `crates/holon/tests/convert_block_to_page_e2e.rs`). The bug: for a RUNTIME
//! convert the new page's identity file was never materialized, so the
//! re-homed child had no on-disk home. Two manifestations of the one gap:
//!   - the origin file's write-back drops the child (it moved to P) and P has
//!     no file -> the child silently vanishes from disk (this test's
//!     assertion),
//!   - or the write-back guard vetoes dropping the child from the origin file
//!     -> render(SQL) != disk PERSISTS -> `inv-org-render-fixed-point` echo
//!     (the keystone signature).
//!
//! Drives the REAL prod convert path (`convert_block_to_page` over the default
//! Loro+Turso wiring, `FileSyncController` write-back). The nested chain is
//! built under the RULE-MINTED journal date page (rooted under the `journals`
//! folder page, exactly like the keystone's frozen-clock 2026-01-15 date), so
//! the new page's `authoritative_name_chain` is a valid all-pages chain — the
//! keystone topology, not a synthetic-root doc.
//!
//! NB: the composed keystone's hand-authored REPLAY path
//! (`hand_authored_regressions`) settles/materializes such that the echo does
//! NOT manifest (a known replay-vs-generated harness divergence), so this
//! app-integration test is the faithful lock, not a JSONL sidecar case.
//!
//! @pbt kind harness
//! @pbt covers convert-block-to-page-materializes-child — a runtime
//!   convert_block_to_page must materialize the new page's identity file so its
//!   re-homed children survive on disk (no render!=disk echo, no data loss)

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

// A 3-deep chain under the journal date page: outer -> middle -> child. The
// middle block (la-1) is the convert origin; its child (la-2) is re-homed under
// the new page.
const CHAIN_ORG: &str = "* la-outer\n:PROPERTIES:\n:ID: la-0\n:END:\n** la-middle\n\
:PROPERTIES:\n:ID: la-1\n:END:\n*** la-grandchild\n:PROPERTIES:\n:ID: la-2\n:END:\n";

#[test]
#[ignore = "RED lock (parked): cross-doc-move source-convergence defect — origin doc cache never sees the departed block; see morning handoff 2026-07-24 + logs locktest-converge-poll.log"]
fn runtime_convert_block_to_page_materializes_rehomed_child_file() {
    let rt = runtime();
    rt.clone().block_on(async {
        // Seed the keystone's actual failing file: a subdir journal date file.
        // Its date-page doc-root is ingested under a synthetic (empty)
        // document-root sentinel, NOT the `journals` folder page — so the date
        // page owns a file (via the alias registry) yet its block_raw ancestor
        // chain contains a non-page. This is the topology under which a runtime
        // convert's new page previously failed to materialize.
        // Pin the boot clock so the daily-journal auto-create rule mints a FIXED
        // "today" page regardless of the real wall-clock date — the test must be
        // deterministic under any date (job 72446a9c: it was clock-sensitive
        // before the cross-doc-move convergence fix). Use a date distinct from
        // the seeded 2026-01-15 subtree under test.
        let pinned_ms = chrono::NaiveDate::from_ymd_opt(2026, 6, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let env = TestEnvironmentBuilder::new()
            .with_clock(Arc::new(holon_api::TestClock::new(pinned_ms)))
            .with_org_file("Journals/2026-01-15.org", CHAIN_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(10))
            .await;
        env.wait_for_org_files_stable(300, Duration::from_secs(10))
            .await;

        // Convert the MIDDLE block into a page. P is minted under the date page;
        // la-2 (its child) is re-homed under P; la-1 becomes a link to P.
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("target".into(), Value::String("block:la-1".into()));
        env.execute_operation("block", "convert_block_to_page", params)
            .await
            .expect("convert_block_to_page dispatch");

        env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(10))
            .await;
        env.wait_for_org_files_stable(300, Duration::from_secs(10))
            .await;

        // The new page P: the sole Page whose content is the origin title.
        let page_rows = env
            .query_sql(
                "SELECT b.id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND t.tag = \
                 'Page' WHERE b.content = 'la-middle'",
            )
            .await
            .expect("query new page");
        let new_page = page_rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| {
                panic!(
                    "convert_block_to_page did not mint a Page for 'la-middle'; rows={page_rows:?}"
                )
            })
            .to_string();

        // The re-homed grandchild must live under P in the store (intended
        // adoption).
        let child_rows = env
            .query_sql("SELECT parent_id FROM block_raw WHERE id = 'block:la-2'")
            .await
            .expect("query child parent");
        let child_parent = child_rows
            .first()
            .and_then(|r| r.get("parent_id"))
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            child_parent, new_page,
            "la-2 should be re-homed under the new page {new_page} (intended adoption)"
        );

        use holon_filesystem::FileSystem;
        let scan = FileSystem::scan_directory(env.org_fs.as_ref(), env.org_root())
            .await
            .expect("scan org dir");
        let page_id_bare = new_page.strip_prefix("block:").unwrap_or(&new_page);
        let mut child_carriers = Vec::new();
        let mut p_file: Option<String> = None;
        let mut dump = String::new();
        for p in &scan.files {
            let content = FileSystem::read_to_string(env.org_fs.as_ref(), p)
                .await
                .unwrap_or_default();
            dump.push_str(&format!("\n--- {} ---\n{content}", p.display()));
            if content.contains("la-grandchild") || content.contains(":ID: la-2") {
                child_carriers.push(p.display().to_string());
            }
            if content.contains(&format!("#+ID: {page_id_bare}")) {
                p_file = Some(p.display().to_string());
            }
        }
        // The new page P must own its identity file, and the re-homed child must
        // live IN that file (nested under the parent page). On the unfixed code
        // P is FILELESS: no `#+ID: <P>` file exists and la-2 either vanished or
        // is stranded in the origin file (guard veto -> render!=disk echo).
        assert!(
            p_file.is_some(),
            "new page {new_page} owns NO identity file on disk — runtime \
             convert_block_to_page did not materialize it (job 72446a9c). Files:{dump}"
        );
        assert_eq!(
            child_carriers,
            vec![p_file.clone().unwrap()],
            "re-homed grandchild la-2 must live in exactly the new page's own file {p_file:?}, \
             not stranded in the origin file nor lost (job 72446a9c). Files:{dump}"
        );
    });
}
