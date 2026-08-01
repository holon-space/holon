//! RED repro for the dogfood 2026-07-10 region-writeback data-loss bug (P0).
//!
//! Shape: a vault with BOTH `Projects/Holon/Frontends.org` (a file, with
//! `#+ID`) AND `Projects/Holon/Frontends/GPUI.org` (a file under a same-named
//! subdir, with `#+ID`). GPUI.org contains a `** Three-mode UX shells` region
//! (`:ID: ba5ad62d-...`) under a same-file parent `* Cross-platform
//! infrastructure`, whose children reference cross-file ids via
//! `:BLOCKED-BY:`/`:REQUIRES:` (`orient-daily-view`, `now-query-mcp`).
//!
//! Bug: the region's blocks ingest without a loud error but never land under
//! the expected document reachable from its `#+ID` root, so org writeback
//! re-renders GPUI.org WITHOUT those lines — silent data loss on disk.
//!
//! Assertion: every `:ID:` present in the source file must (a) be present in
//! the DB reachable from the GPUI document root after boot, and (b) survive in
//! the re-rendered file on disk.
//!
//! Two further cases pin the write-back's two exits when a file's blocks do NOT
//! all land under it: a PARTIAL ingest (`Err`) must quarantine the file so its
//! truncated DB state is never rendered over disk, while a fully-absorbed
//! cross-doc stale copy (`Ok`) must be pruned — and neither may take an
//! authored line with it.
//!
//! @pbt kind harness
//! @pbt covers region-writeback-loss — region-writeback data-loss P0 repro
//! (dogfood 2026-07-10)

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;

/// Runtime whose workers carry this case's observability scope, so an `error!`
/// emitted on a tokio worker — not the test thread — is attributed here and
/// readable through `captured_problems`.
///
/// `SpanCollector::global()` is the SOLE subscriber installer for this file:
/// its `init()` panics if anything else claimed the global default first, and
/// these tests run concurrently in one process.
fn runtime() -> Arc<tokio::runtime::Runtime> {
    SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    Arc::new(builder.build().expect("Failed to create runtime"))
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
        "[{phase}] region data loss:\n  DB-unreachable-from-doc {GPUI_DOC_ID}: {db_missing:?}\n  \
         missing-on-disk-after-writeback: {disk_missing:?}\n  parent_of (bare): {parent_of:?}\n  \
         disk content:\n{disk_gpui}",
    );
}

#[test]
fn three_mode_region_survives_ingest_and_writeback() {
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
            "{GPUI_ORG}\n* Appended\n:PROPERTIES:\n:ID: appended-block\n:END:\nPlaceholder \
             appended text.\n"
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

/// Every ERROR-level event captured for this case, joined.
fn captured_errors() -> String {
    SpanCollector::global()
        .captured_problems()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// True when a SINGLE captured ERROR is both the write-back quarantine
/// disclosure AND names `file_name`. Scoped to one problem on purpose: matching
/// the disclosure anywhere in the joined errors would be satisfied by a future
/// prod change that quarantines some OTHER fixture file, silently restoring the
/// vacuity this premise check exists to prevent. The layer folds the event's
/// `path` field into the captured message, so the file name is available here.
fn quarantine_disclosed_for(file_name: &str) -> bool {
    SpanCollector::global().captured_problems().iter().any(|p| {
        let text = p.to_string();
        text.contains("QUARANTINING this file from write-back") && text.contains(file_name)
    })
}

/// Poll until `cond` holds, up to `SYNC_TIMEOUT`. `wait_for_org_files_stable`
/// cannot serve here: a file the watcher has not picked up yet is trivially
/// "stable", so it returns before the ingest under test has even started.
/// Returns whether the condition was reached, so the caller still asserts.
async fn wait_until(mut cond: impl AsyncFnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        if cond().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert every authored `:ID:` and every authored body line is still on disk.
/// This is the real anti-data-loss contract: it survives a SANCTIONED rewrite
/// (stale-prune, a gained `#+ID:` header, property reordering) yet still fails
/// on any truncation.
fn assert_nothing_lost(authored: &str, on_disk: &str, label: &str) {
    let disk_ids: HashSet<String> = block_ids(on_disk).into_iter().collect();
    let missing_ids: Vec<String> = block_ids(authored)
        .into_iter()
        .filter(|id| !disk_ids.contains(id))
        .collect();
    let disk_lines: HashSet<&str> = on_disk.lines().map(str::trim_end).collect();
    let missing_lines: Vec<&str> = authored
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !disk_lines.contains(l))
        .collect();
    assert!(
        missing_ids.is_empty() && missing_lines.is_empty(),
        "[{label}] data loss on disk:\n  missing :ID:s: {missing_ids:?}\n  missing lines: \
         {missing_lines:?}\n  on disk:\n{on_disk}",
    );
}

// ── Write-back quarantine guard ─────────────────────────────────────────────
// A file whose ingest returns `Err` must never be rewritten from its (at best
// partial) DB state.
//
// Trigger: a SPLIT DOC ROOT — a file whose declared `#+ID:` anchor is disjoint
// from the subtree its blocks actually live in, plus an ID-less headline that
// would have to be minted under the foreign anchor.
// `assert_mint_parents_inside_doc_anchor` (`file_sync_controller.rs:1623`)
// refuses the ingest rather than re-minting a fresh id on every pass, and the
// `Err` reaches `on_file_changed`'s quarantine arm (`:1513-1531`).
//
// The cross-doc membership guard cannot absorb this: that guard prunes a stale
// copy of an ALREADY-OWNED block, while the refusal here is about where NEW
// ID-less headlines would be minted — a decision the guard never reaches.

const SPLIT_HOLON: &str = "11111111-1111-4111-8111-111111111111";
const SPLIT_TASKS: &str = "22222222-2222-4222-8222-222222222222";
/// The UNTAGGED headline that owns the real content.
const SPLIT_PPU_INLINE: &str = "33333333-3333-4333-8333-333333333333";
/// The headline both files claim; it stays under `SPLIT_PPU_INLINE`.
const SPLIT_FIXBUGS: &str = "44444444-4444-4444-8444-444444444444";
/// The page-file's declared `#+ID:` anchor — a `Page` sibling under `Holon`.
const SPLIT_PPU_PAGE: &str = "55555555-5555-4555-8555-555555555555";
/// The anchor's sole child, itself a page, which empties the anchor's walk.
const SPLIT_DELME: &str = "66666666-6666-4666-8666-666666666666";

const SPLIT_CHAIN_PATH: &str = "Projects/Holon.org";
const SPLIT_PAGE_PATH: &str = "Projects/Holon/Prepare personal usage.org";
const SPLIT_DELME_PATH: &str = "Projects/Holon/Prepare personal usage/DeleteMe.org";

fn split_chain_org() -> String {
    format!(
        "#+ID: {SPLIT_HOLON}\n#+TITLE: Holon\n\n\
         * Tasks\n:PROPERTIES:\n:ID: {SPLIT_TASKS}\n:END:\n\
         ** Prepare personal usage\n:PROPERTIES:\n:ID: {SPLIT_PPU_INLINE}\n:END:\n\
         *** Fix bugs\n:PROPERTIES:\n:ID: {SPLIT_FIXBUGS}\n:END:\n"
    )
}

/// The split-root page-file: its anchor is `SPLIT_PPU_PAGE`, but the block it
/// authors lives under `SPLIT_PPU_INLINE`. The ID-less `** Probe` headline is
/// what forces the mint decision, and its body must survive the refusal.
fn split_page_org() -> String {
    format!(
        "#+ID: {SPLIT_PPU_PAGE}\n#+TITLE: Prepare personal usage\n\n\
         * Fix bugs\n:PROPERTIES:\n:ID: {SPLIT_FIXBUGS}\n:END:\n\
         ** Probe\nProbe body that must survive on disk.\n"
    )
}

/// The refused ingest must leave every authored `:ID:` and body line on disk.
#[test]
fn partial_ingest_does_not_rewrite_the_file() {
    let rt = runtime();
    rt.clone().block_on(async {
        // Boot the chain alone so `Fix bugs` is already owned by the chain's
        // document, then let the split-root page-file appear — the order the
        // live vault reached this state.
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file(SPLIT_CHAIN_PATH, split_chain_org())
            .build(rt.clone())
            .await
            .expect("boot");
        assert!(
            env.wait_for_block(SPLIT_FIXBUGS, SYNC_TIMEOUT).await,
            "precondition: the chain must ingest first"
        );
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let delme_path = env.org_file_path(SPLIT_DELME_PATH);
        env.org_fs
            .mkdir_all(delme_path.parent().expect("DeleteMe.org has a parent dir"));
        FileSystem::write(
            env.org_fs.as_ref(),
            &delme_path,
            format!("#+ID: {SPLIT_DELME}\n#+TITLE: DeleteMe\n").as_bytes(),
        )
        .await
        .expect("write DeleteMe.org");
        // Sequenced, not raced: the anchor's sole child must already be a page
        // when the split-root file is ingested — that is what empties the
        // anchor's candidate read and forces the mint decision.
        assert!(
            env.wait_for_block(SPLIT_DELME, SYNC_TIMEOUT).await,
            "precondition: DeleteMe.org must ingest before the split-root file"
        );
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let page_path = env.org_file_path(SPLIT_PAGE_PATH);
        FileSystem::write(env.org_fs.as_ref(), &page_path, split_page_org().as_bytes())
            .await
            .expect("write split-root page file");
        // (1) The premise: the ingest really was refused and the file really is
        // quarantined. Without this, (2) would also hold for a file that simply
        // ingested cleanly, and the test would prove nothing.
        let quarantined =
            wait_until(async || quarantine_disclosed_for("Prepare personal usage.org")).await;
        assert!(
            quarantined,
            "expected the write-back quarantine disclosure to NAME the split-root page file — \
             without it the fixture no longer stages a refused ingest (and a disclosure about some \
             OTHER file must not satisfy this). Captured ERRORs:\n{}",
            captured_errors()
        );
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let on_disk = env
            .org_fs
            .read_to_string(&page_path)
            .await
            .expect("read split-root page file");

        // (2) The contract: nothing the file authored is gone from disk. Stated
        // as `:ID:`/body survival rather than byte-equality, so a SANCTIONED
        // rewrite (stale-prune, a gained `#+ID:` header) still passes while any
        // truncation fails.
        assert_nothing_lost(&split_page_org(), &on_disk, "quarantined-refused-ingest");
    });
}

// ── Foreign page inline: silent block loss (BugFunnel 2026-08-01) ───────────
// RED, disclosed. A folder-companion that INLINES a foreign page root has that
// headline AND every block beneath it deleted from disk on write-back, while
// those blocks land in NO store row and NO other file, and NOTHING is logged.
// `check_writeback_lossless` does not fire, so the quarantine above never
// engages. See the BugFunnel row dated 2026-08-01.

/// The page-file's `#+ID:` — the id the companion below inlines.
const INLINED_PAGE_ID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
/// The folder-companion's own `#+ID:`.
const COMPANION_DOC_ID: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

const PAGE_FILE_PATH: &str = "Projects/Frontends/GPUI.org";
const COMPANION_PATH: &str = "Projects/Frontends.org";

fn page_file() -> String {
    format!(
        "#+ID: {INLINED_PAGE_ID}\n#+TITLE: GPUI\n\n\
         * Owned By The Page File\n:PROPERTIES:\n:ID: page-owned-child\n:END:\n\
         Body owned by the page file.\n"
    )
}

/// The companion inlines the page root `INLINED_PAGE_ID` and adds blocks of its
/// own around it. `inlined-descendant` is authored ONLY here — it belongs to no
/// other file — so nothing else can carry it if this file loses it.
fn companion_file() -> String {
    format!(
        "#+ID: {COMPANION_DOC_ID}\n#+TITLE: Frontends\n\n\
         * Companion Head\n:PROPERTIES:\n:ID: companion-head\n:END:\n\
         Companion head body that must survive on disk.\n\n\
         * GPUI\n:PROPERTIES:\n:ID: {INLINED_PAGE_ID}\n:END:\n\
         Inlined copy of the page root.\n\n\
         ** Inlined Descendant\n:PROPERTIES:\n:ID: inlined-descendant\n:END:\n\
         Inlined descendant body.\n\n\
         * Companion Tail\n:PROPERTIES:\n:ID: companion-tail\n:END:\n\
         Companion tail body that must survive on disk.\n"
    )
}

/// Boot the page-file alone, then let the companion appear — so the page root
/// is already store-resident and `Page`-tagged when the companion is ingested,
/// which is what makes its inlined copy a FOREIGN page root.
async fn companion_env(
    rt: Arc<tokio::runtime::Runtime>,
) -> holon_integration_tests::TestEnvironment {
    let env = TestEnvironmentBuilder::new()
        .without_loro()
        .with_org_file(PAGE_FILE_PATH, page_file())
        .build(rt)
        .await
        .expect("boot");
    assert!(
        env.wait_for_block("page-owned-child", SYNC_TIMEOUT).await,
        "precondition: the page-file must ingest first"
    );
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

    let companion_path = env.org_file_path(COMPANION_PATH);
    FileSystem::write(
        env.org_fs.as_ref(),
        &companion_path,
        companion_file().as_bytes(),
    )
    .await
    .expect("write companion");
    // The companion's OWN blocks landing is the signal that its ingest ran;
    // only then is the write-back's treatment of the inlined subtree decided.
    assert!(
        env.wait_for_block("companion-tail", SYNC_TIMEOUT).await,
        "precondition: the companion's own blocks must ingest"
    );
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
    env
}

/// Blocks authored under an inlined foreign page root must not vanish: either
/// this file keeps them, or the owning page-file adopts them, or the ingest is
/// refused and quarantined. Today none of the three happens.
#[test]
fn companion_inlining_a_foreign_page_root_keeps_its_blocks() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = companion_env(rt.clone()).await;

        let on_disk = env
            .org_fs
            .read_to_string(&env.org_file_path(COMPANION_PATH))
            .await
            .expect("read companion");
        let page_disk = env
            .org_fs
            .read_to_string(&env.org_file_path(PAGE_FILE_PATH))
            .await
            .expect("read page file");
        let store_ids: HashSet<String> = env
            .non_page_block_rows()
            .await
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .collect();

        // The loss is SILENT: no refusal, no quarantine, nothing to alert on.
        // Asserted first so a future fix that merely starts disclosing the loss
        // is not mistaken for a fix that prevents it.
        let errors = captured_errors();
        assert!(
            errors.is_empty(),
            "unexpected disclosure — if the loss is now reported, this test's framing needs \
             revisiting:\n{errors}"
        );

        // `inlined-descendant` is authored only by the companion. It must
        // survive SOMEWHERE: this file, the owning page-file, or the store.
        let survives = on_disk.contains("inlined-descendant")
            || page_disk.contains("inlined-descendant")
            || store_ids.contains("block:inlined-descendant");
        assert!(
            survives,
            "silent data loss: `inlined-descendant` was authored under the inlined foreign page \
             root and is now in NO store row, NO other file, and deleted from its own file.\n  \
             companion on disk:\n{on_disk}\n  page-file on disk:\n{page_disk}",
        );
        assert_nothing_lost(&companion_file(), &on_disk, "foreign-page-inline");
    });
}

// ── Sanctioned cross-doc prune ──────────────────────────────────────────────
// The complement of the quarantine: an ingest that the cross-doc membership
// guard fully absorbs is NOT a partial ingest. It returns `Ok`, so the file is
// re-rendered — legitimately WITHOUT the stale copy of the foreign-owned block,
// which converges to its real owner. Everything this file actually owns must
// survive that rewrite.

/// `aaa_owner.org` OWNS `block:shared-child` under its own root.
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

/// `zzz_stale.org` carries a stale on-disk copy of `shared-child`. Its own
/// blocks (`stale-top`, the parent, `stale-tail`) must survive the prune.
const STALE_FILE: &str = "\
* Stale Top
:PROPERTIES:
:ID: stale-top
:END:
Placeholder body for stale-top.

** Parent
:PROPERTIES:
:ID: 77777777-7777-7777-7777-777777777777
:END:
*** Shared
:PROPERTIES:
:ID: shared-child
:END:

* Stale Tail
:PROPERTIES:
:ID: stale-tail
:END:
Placeholder body that must survive on disk.
";

/// `STALE_FILE` minus the foreign-owned `Shared` headline — everything the
/// prune is NOT allowed to take with it.
const STALE_OWNED: &str = "\
* Stale Top
:PROPERTIES:
:ID: stale-top
:END:
Placeholder body for stale-top.

** Parent
:PROPERTIES:
:ID: 77777777-7777-7777-7777-777777777777
:END:

* Stale Tail
:PROPERTIES:
:ID: stale-tail
:END:
Placeholder body that must survive on disk.
";

#[test]
fn writeback_stale_cross_doc_prune() {
    let rt = runtime();
    rt.clone().block_on(async {
        // Alphabetical scan order puts `aaa_owner.org` first, so `shared-child`
        // is already store-resident (and owned by `other-root`'s document) when
        // `zzz_stale.org` is ingested — exactly the condition the cross-doc
        // membership guard fires on.
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("aaa_owner.org", OWNER_FILE)
            .with_org_file("zzz_stale.org", STALE_FILE)
            .build(rt.clone())
            .await
            .expect("boot");
        assert!(
            env.wait_for_block("stale-top", SYNC_TIMEOUT).await,
            "the stale file's own blocks must ingest"
        );
        let stale_path = env.org_file_path("zzz_stale.org");

        // The foreign-owned copy is pruned from this file's write-back so it
        // converges to its real owner. Waited on rather than slept for — the
        // prune IS the write-back this test is about.
        let pruned = wait_until(async || {
            !env.org_fs
                .read_to_string(&stale_path)
                .await
                .expect("read stale")
                .contains("shared-child")
        })
        .await;
        let on_disk = env
            .org_fs
            .read_to_string(&stale_path)
            .await
            .expect("read stale");
        assert!(
            pruned,
            "the stale cross-doc copy must be pruned from this file's write-back so it converges \
             to its real owner. On disk:\n{on_disk}"
        );

        // The guard ABSORBED it: an absorbed stale copy is not a partial
        // ingest, so THIS file must not be quarantined. Scoped to the file
        // under test so an unrelated quarantine elsewhere cannot fail it.
        assert!(
            !quarantine_disclosed_for("zzz_stale.org"),
            "a cross-doc stale copy is absorbed by the membership guard, not a partial ingest — \
             it must NOT quarantine. Captured ERRORs:\n{}",
            captured_errors()
        );
        // ...and NOTHING this file owns is lost with it.
        assert_nothing_lost(STALE_OWNED, &on_disk, "cross-doc-prune");
    });
}
