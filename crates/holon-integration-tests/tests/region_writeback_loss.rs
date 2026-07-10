//! RED repro for the dogfood 2026-07-10 region-writeback data-loss bug (P0).
//!
//! Shape: a vault with BOTH `Projects/Holon/Frontends.org` (a file, with `#+ID`)
//! AND `Projects/Holon/Frontends/GPUI.org` (a file under a same-named subdir,
//! with `#+ID`). GPUI.org contains a `** Three-mode UX shells` region
//! (`:ID: ba5ad62d-...`) under a same-file parent `* Cross-platform
//! infrastructure`, whose children reference cross-file ids via
//! `:BLOCKED-BY:`/`:REQUIRES:` (`orient-daily-view`, `now-query-mcp`).
//!
//! Bug: the region's blocks ingest without a loud error but never land under
//! the expected document reachable from its `#+ID` root, so org writeback
//! re-renders GPUI.org WITHOUT those lines — silent data loss on disk.
//!
//! Assertion: every `:ID:` present in the source file must (a) be present in the
//! DB reachable from the GPUI document root after boot, and (b) survive in the
//! re-rendered file on disk.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(15);

const FRONTENDS_ORG: &str = include_str!("fixtures/region_writeback_loss/Frontends.org");
const GPUI_ORG: &str = include_str!("fixtures/region_writeback_loss/GPUI.org");

const GPUI_DOC_ID: &str = "d09025cc-3748-404e-ad4d-432fcdc194d5";

/// Every `:ID:` in a `:PROPERTIES:` drawer of the given org text (the block ids
/// that MUST survive ingest + writeback).
fn block_ids(org: &str) -> Vec<String> {
    org.lines()
        .filter_map(|l| l.trim().strip_prefix(":ID:"))
        .map(|id| id.trim().to_string())
        .collect()
}

/// Build parent_id -> children and check which of `ids` are reachable from the
/// `root` doc id by walking parent_id links present in `rows`.
fn reachable_from_root(
    rows: &[holon_api::StorageEntity],
    root: &str,
) -> (HashSet<String>, HashMap<String, String>) {
    // id -> parent_id (bare)
    let mut parent_of: HashMap<String, String> = HashMap::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .trim_start_matches("block:")
            .to_string();
        let parent = row
            .get("parent_id")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .trim_start_matches("block:")
            .to_string();
        parent_of.insert(id, parent);
    }
    let mut reachable = HashSet::new();
    for id in parent_of.keys() {
        let mut cur = id.clone();
        let mut hops = 0;
        loop {
            let Some(p) = parent_of.get(&cur) else { break };
            if p == root {
                reachable.insert(id.clone());
                break;
            }
            if p == &cur || hops > 200 {
                break;
            }
            cur = p.clone();
            hops += 1;
        }
    }
    (reachable, parent_of)
}

fn assert_region_survives(rows: &[holon_api::StorageEntity], disk_gpui: &str, phase: &str) {
    let expected_ids = block_ids(GPUI_ORG);
    let (reachable, parent_of) = reachable_from_root(rows, GPUI_DOC_ID);

    // (a) DB reachability from the GPUI doc root.
    let db_missing: Vec<&String> = expected_ids
        .iter()
        .filter(|id| !reachable.contains(*id))
        .collect();

    // (b) survival on disk after writeback.
    let disk_ids: HashSet<String> = block_ids(disk_gpui).into_iter().collect();
    let disk_missing: Vec<&String> = expected_ids
        .iter()
        .filter(|id| !disk_ids.contains(*id))
        .collect();

    assert!(
        db_missing.is_empty() && disk_missing.is_empty(),
        "[{phase}] region data loss:\n  \
         DB-unreachable-from-doc {GPUI_DOC_ID}: {db_missing:?}\n  \
         missing-on-disk-after-writeback: {disk_missing:?}\n  \
         parent_of (bare): {parent_of:?}\n  \
         disk content:\n{disk_gpui}",
    );
}

#[test]
fn three_mode_region_survives_ingest_and_writeback() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Projects/Holon/Frontends.org", FRONTENDS_ORG)
            .with_org_file("Projects/Holon/Frontends/GPUI.org", GPUI_ORG)
            .build(rt.clone())
            .await
            .expect("boot");

        // A block deep in the ba5ad62d subtree must ingest.
        assert!(
            env.wait_for_block("capture-mode-overlay", SYNC_TIMEOUT)
                .await,
            "ba5ad62d child must ingest on boot"
        );

        let path = env.org_file_path("Projects/Holon/Frontends/GPUI.org");

        // --- Initial scan (CREATE path): every :ID: reachable from the doc root
        // and every :ID: still on disk after the forced write-back. Before the
        // fix, `flow-mode-shell`'s create transaction hit the `block_requires`
        // `required_id` FK (target `orient-daily-view` not yet created) and the
        // whole file scan aborted, dropping the region + everything after it.
        let rows = env.non_page_block_rows().await;
        let disk = env.org_fs.read_to_string(&path).await.expect("read gpui");
        assert_region_survives(&rows, &disk, "initial-scan");

        // --- Re-ingest (UPDATE path): rewrite the file (append a block). The
        // existing blocks now take the update branch, whose edge write is a
        // DELETE+INSERT of the same `block_requires` rows — the soft-target
        // reference must survive a re-ingest too.
        let appended = format!(
            "{GPUI_ORG}\n* Appended\n:PROPERTIES:\n:ID: appended-block\n:END:\nPlaceholder appended text.\n"
        );
        env.write_org_file("Projects/Holon/Frontends/GPUI.org", &appended)
            .await
            .expect("rewrite gpui");
        assert!(
            env.wait_for_block("appended-block", SYNC_TIMEOUT).await,
            "appended block must ingest on re-scan"
        );
        let rows2 = env.non_page_block_rows().await;
        let disk2 = env.org_fs.read_to_string(&path).await.expect("read gpui 2");
        assert_region_survives(&rows2, &disk2, "re-ingest");
    });
}

// ── Write-back quarantine guard ─────────────────────────────────────────────
// A file whose ingest fails PARTWAY must never be rewritten from its truncated
// DB state. This is defense-in-depth independent of any single root cause: force
// a partial ingest (a block whose id is OWNED by another document, re-parented
// across docs during the scan — the deterministic partial-ingest stand-in from
// `boot_scan_bad_file_survives`) and assert the on-disk file is byte-unchanged.

// `owner.org` OWNS block:shared-child under its own root.
const OWNER_FILE: &str = "\
* Other Root
:PROPERTIES:
:ID: other-root
:END:
** Shared
:PROPERTIES:
:ID: shared-child
:END:
";

// `bad.org` lands `bad-top` (a create → fires CDC), then fails re-parenting
// `shared-child` (owned by owner.org) across documents — a partial ingest. The
// `bad-tail` line after it must NOT be lost from disk by a write-back rendering
// the (prefix-only) DB state.
const BAD_FILE: &str = "\
* Bad Top
:PROPERTIES:
:ID: bad-top
:END:
Placeholder body for bad-top.

** Parent
:PROPERTIES:
:ID: 66666666-6666-6666-6666-666666666666
:END:
*** Shared
:PROPERTIES:
:ID: shared-child
:END:

* Bad Tail
:PROPERTIES:
:ID: bad-tail
:END:
Placeholder body that must survive on disk.
";

#[test]
fn partial_ingest_does_not_rewrite_the_file() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        // Default (Loro) wiring: the cross-document re-parent of `shared-child`
        // is rejected by Loro's `resolve_parent_tree_id` mid-scan — a
        // deterministic partial ingest (same stand-in as
        // `boot_scan_bad_file_survives`). SqlOnly would instead succeed the
        // re-parent (plain `parent_id` column write), so it can't stage a partial
        // ingest here; the guard itself is mode-agnostic.
        let env = TestEnvironmentBuilder::new()
            .with_org_file("aaa_owner.org", OWNER_FILE)
            .with_org_file(
                "mmm_good.org",
                "* Good\n:PROPERTIES:\n:ID: good-root\n:END:\n",
            )
            .with_org_file("zzz_bad.org", BAD_FILE)
            .build(rt.clone())
            .await
            .expect("boot must survive one bad file");

        // Survival: the clean file still ingests despite the bad file.
        assert!(
            env.wait_for_block("good-root", SYNC_TIMEOUT).await,
            "clean file must still ingest (a bad file must not kill sync)"
        );
        // Let any CDC-triggered re-render settle.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The bad file's ingest failed partway → it is quarantined from
        // write-back → its on-disk bytes are left EXACTLY as authored. Before the
        // guard, the CDC events from the landed prefix (`bad-top`) re-rendered the
        // doc from its truncated DB state and wrote it back, dropping `bad-tail`.
        let bad_path = env.org_file_path("zzz_bad.org");
        let on_disk = env
            .org_fs
            .read_to_string(&bad_path)
            .await
            .expect("read bad");
        assert_eq!(
            on_disk, BAD_FILE,
            "quarantined (partially-ingested) file was rewritten — data loss. \
             Every :ID: must remain on disk. Got:\n{on_disk}"
        );
    });
}
