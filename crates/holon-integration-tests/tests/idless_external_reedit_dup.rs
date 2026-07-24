//! Regression pin for the dogfood 2026-07-22 ID-less external-re-edit
//! duplicate/identity-churn bug (BugFunnel / PR #76).
//!
//! Symptom (observed at ~60-block scale on the live vault): an external editor
//! bulk-writes an org file whose headlines carry NO `:ID:`. The running app
//! ingests it, mints a UUID per headline, and writes the file back with `:ID:`
//! drawers. A SECOND external write derived from the *pre-mint* text (the
//! classic editor/agent write-then-follow-up-edit workflow) re-ingests the
//! still-ID-less headlines. Because every ID-less headline is minted a FRESH
//! `Uuid::new_v4()` on every parse (`parser::extract_or_generate_id`), the
//! re-ingest's ids never match the already-minted twins — and the reconciler,
//! which keyed UPDATE-vs-CREATE by block-id ONLY, re-minted each headline: its
//! identity CHURNED (all references to the old id break), and under a
//! concurrent writeback (base desync) the old twin survived, DUPLICATING the
//! block.
//!
//! Root cause (single, deterministic here): the re-parse mints a new id that
//! misses the by-id `old_blocks` lookup. The observable-on-a-single-thread
//! facet is IDENTITY CHURN — the headline's id changes on every stale re-write.
//! The duplicate is the same root cause manifesting when the diff base has
//! desynced from the store (concurrent-writeback timing; ENVIRONMENT secondary,
//! not reproducible single-threaded — noted as a keystone follow-up in the
//! BugFunnel row).
//!
//! Remedy under test: before minting, an ID-less incoming headline is
//! reconciled onto its already-minted twin among the STORE's current children
//! by exact CONTENT + sibling POSITION + a STRUCTURAL SIGNATURE OF DESCENDANTS
//! (`FileSyncController::ingest_file` → `TieredMatcher`), so the stale re-write
//! reconciles onto the existing block — its id stays STABLE and it is never
//! duplicated.
//!
//! RULING A2 (2026-07-24) tightened the tie-break for AMBIGUOUS-content blocks
//! (same content, neither position-exact nor content-unique). Where the matcher
//! once minted a fresh id ("MintAmbiguous", tolerating a duplicate), it now
//! PAIRS the incoming id-less twins onto the existing twins deterministically:
//! by descendant subtree signature first (so children can never be silently
//! re-homed onto the WRONG identical-content twin), then by relative sibling
//! position (identical-subtree twins are interchangeable — either pairing is
//! correct). Fresh-mint remains ONLY for a genuinely new block (no unclaimed
//! same-content candidate left). So identical-content siblings stop duplicating
//! even when a reorder shifts their positions, and identical-content PARENTS
//! keep their own children across an external reorder.
//!
//! These tests live here (not in the pure parse↔render PBT) because the bug is
//! in the ingest reconcile of a *running* `FileSyncController` — the
//! external-write → app-writeback → stale-external-rewrite → re-ingest cycle.
//! It is a candidate keystone transition (an external-editor-write rung that
//! emits ID-less text, lets the app writeback, then re-emits the pre-mint text)
//! — noted in the BugFunnel row.
//!
//! @pbt kind harness
//! @pbt covers idless-external-reedit-dup — ID-less headline re-ingest must not
//! churn identity or duplicate (dogfood 2026-07-22, PR #76)

use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// All store block ids whose `content` equals `content` exactly, sorted.
async fn ids_with_content(
    env: &holon_integration_tests::TestEnvironment,
    content: &str,
) -> Vec<String> {
    let rows = env
        .query(
            &format!("from block | filter content == \"{content}\" | select {{id, content}}"),
            QueryLanguage::HolonPrql,
        )
        .await
        .expect("id query failed");
    let mut ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// The `parent_id` of the (unique) store block whose `content` equals
/// `content` exactly. Panics if there is not exactly one such block.
async fn parent_of_content(
    env: &holon_integration_tests::TestEnvironment,
    content: &str,
) -> String {
    let rows = env
        .query(
            &format!("from block | filter content == \"{content}\" | select {{id, parent_id}}"),
            QueryLanguage::HolonPrql,
        )
        .await
        .expect("parent query failed");
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one block with content {content:?}, got {rows:?}"
    );
    rows[0]
        .get("parent_id")
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("block {content:?} has no parent_id: {rows:?}"))
}

/// Poll the file on disk until `pred(content)` holds or the timeout elapses.
async fn wait_for_disk<F: Fn(&str) -> bool>(
    env: &holon_integration_tests::TestEnvironment,
    path: &std::path::Path,
    pred: F,
    label: &str,
) {
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let content = env.org_fs.read_to_string(path).await.expect("read");
        if pred(&content) {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for disk condition [{label}]:\n{content}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The deterministic root-cause pin: an ID-less headline ingested (mints +
/// writes back `:ID:`), then the pre-mint (still ID-less) text is re-written
/// externally. After the re-ingest there must still be exactly ONE block for
/// the headline AND its id must be UNCHANGED — before the fix the re-parse
/// minted a fresh uuid, churning the block's identity (and, on base desync,
/// duplicating it).
#[test]
fn idless_headline_reingest_preserves_identity() {
    let rt = runtime();
    rt.block_on(async {
        const HEADLINE: &str = "Prepare personal usage";
        // No `:PROPERTIES:`/`:ID:` — a bare headline, as an external editor or
        // agent emits before Holon has assigned identity.
        let idless = format!("* {HEADLINE}\n");

        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Notes.org", idless.clone())
            .build(rt.clone())
            .await
            .expect("boot");

        let path = env.org_file_path("Notes.org");

        // First ingest mints a UUID and writes it back as an `:ID:` drawer.
        wait_for_disk(&env, &path, |c| c.contains(":ID:"), "first writeback (mint)").await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let before = ids_with_content(&env, HEADLINE).await;
        assert_eq!(
            before.len(),
            1,
            "after the first ingest there must be exactly one block for the headline, got {before:?}"
        );

        // A stale external editor re-writes the PRE-MINT (ID-less) text,
        // clobbering the app's `:ID:` writeback.
        FileSystem::write(env.org_fs.as_ref(), &path, idless.as_bytes())
            .await
            .expect("stale re-write");

        // The re-ingest writes `:ID:` back a second time — wait for that, then
        // let the controller settle.
        wait_for_disk(&env, &path, |c| c.contains(":ID:"), "second writeback (re-ingest)").await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let after = ids_with_content(&env, HEADLINE).await;
        assert_eq!(
            after.len(),
            1,
            "ID-less re-ingest must not duplicate the headline: got {after:?}"
        );
        assert_eq!(
            after, before,
            "ID-less re-ingest must reconcile onto the existing block, preserving its id \
             (identity churn — before {before:?}, after {after:?})"
        );
    });
}

/// The flagged caveat: two genuinely-distinct ID-less siblings with IDENTICAL
/// content must NOT be merged into one. The content+position reconcile is
/// positional 1:1, so both survive as separate blocks — with stable ids —
/// across a stale re-ingest.
#[test]
fn two_identical_idless_siblings_survive_reingest() {
    let rt = runtime();
    rt.block_on(async {
        const HEADLINE: &str = "Recurring reminder";
        let two = format!("* {HEADLINE}\n* {HEADLINE}\n");

        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Dupes.org", two.clone())
            .build(rt.clone())
            .await
            .expect("boot");

        let path = env.org_file_path("Dupes.org");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains(":ID:"),
            "first writeback (mint)",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let before = ids_with_content(&env, HEADLINE).await;
        assert_eq!(
            before.len(),
            2,
            "two identical ID-less siblings must ingest as two distinct blocks, got {before:?}"
        );

        // Re-write the pre-mint (ID-less) text: the two siblings must reconcile
        // 1:1 onto their two minted twins — still exactly two, neither merged to
        // one nor duplicated to four, with the same ids.
        FileSystem::write(env.org_fs.as_ref(), &path, two.as_bytes())
            .await
            .expect("stale re-write");
        wait_for_disk(&env, &path, |c| c.contains(":ID:"), "second writeback").await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let after = ids_with_content(&env, HEADLINE).await;
        assert_eq!(
            after.len(),
            2,
            "two identical ID-less siblings must stay two blocks across a stale re-ingest, got \
             {after:?}"
        );
        assert_eq!(
            after, before,
            "the two siblings must reconcile 1:1 onto their twins, preserving both ids (before \
             {before:?}, after {after:?})"
        );
    });
}

/// RULING A2 (a): two identical-content ID-less siblings whose POSITIONS SHIFT
/// on re-ingest (a genuinely-new headline is inserted ahead of them) must still
/// reconcile onto their existing twins — identities preserved, no duplicate.
///
/// The position shift defeats the T1 exact-position tie-break for the trailing
/// twin, so pre-A2 it fell to MintAmbiguous: it minted a fresh id and the
/// orphaned twin was deleted (identity CHURN) — after != before. A2 pairs it by
/// subtree signature (both leaves ⇒ interchangeable ⇒ relative position), so
/// both twins keep their ids and the inserted headline mints fresh.
#[test]
fn shifted_identical_twins_preserve_identity() {
    let rt = runtime();
    rt.block_on(async {
        const DUP: &str = "Weekly review";
        let two = format!("* {DUP}\n* {DUP}\n");

        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Shift.org", two.clone())
            .build(rt.clone())
            .await
            .expect("boot");

        let path = env.org_file_path("Shift.org");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains(":ID:"),
            "first writeback (mint)",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let before = ids_with_content(&env, DUP).await;
        assert_eq!(
            before.len(),
            2,
            "two twins ingest as two blocks, got {before:?}"
        );

        // A NEW headline is inserted ahead of the twins (still all ID-less):
        // positions shift 0,1 → 1,2, defeating positional matching for the last.
        let shifted = format!("* Sprint kickoff\n* {DUP}\n* {DUP}\n");
        FileSystem::write(env.org_fs.as_ref(), &path, shifted.as_bytes())
            .await
            .expect("stale re-write with insert");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains("Sprint kickoff"),
            "second writeback",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let after = ids_with_content(&env, DUP).await;
        assert_eq!(
            after.len(),
            2,
            "shifted identical twins must stay two blocks (no duplicate/orphan), got {after:?}"
        );
        assert_eq!(
            after, before,
            "shifted identical twins must reconcile onto their existing twins, preserving both \
             ids (before {before:?}, after {after:?})"
        );
        let fresh = ids_with_content(&env, "Sprint kickoff").await;
        assert_eq!(
            fresh.len(),
            1,
            "the inserted headline mints exactly one fresh block"
        );
    });
}

/// RULING A2 (b): two identical-content PARENTS with DIFFERENT children, then
/// an external reorder SWAPS the two parents. Their children must follow their
/// OWN parent identity — never re-homed onto the wrong same-content twin. The
/// descendant subtree signature discriminates the twins; positional matching
/// alone would swap the children.
#[test]
fn swapped_identical_parents_keep_their_children() {
    let rt = runtime();
    rt.block_on(async {
        // Two `Item` parents, distinct children Alpha / Beta.
        let tree = "* Item\n** Alpha\n* Item\n** Beta\n".to_string();

        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Swap.org", tree)
            .build(rt.clone())
            .await
            .expect("boot");

        let path = env.org_file_path("Swap.org");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains(":ID:"),
            "first writeback (mint)",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let alpha_parent_before = parent_of_content(&env, "Alpha").await;
        let beta_parent_before = parent_of_content(&env, "Beta").await;
        assert_ne!(
            alpha_parent_before, beta_parent_before,
            "the two Item parents must be distinct blocks"
        );

        // External reorder: SWAP the two parents (still all ID-less). Positional
        // matching would map incoming-parent-0 (now carrying Beta) onto the
        // store parent that had Alpha — re-homing the children.
        let swapped = "* Item\n** Beta\n* Item\n** Alpha\n".to_string();
        FileSystem::write(env.org_fs.as_ref(), &path, swapped.as_bytes())
            .await
            .expect("stale re-write (swap)");
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
        wait_for_disk(&env, &path, |c| c.contains(":ID:"), "second writeback").await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let alpha_parent_after = parent_of_content(&env, "Alpha").await;
        let beta_parent_after = parent_of_content(&env, "Beta").await;
        assert_eq!(
            alpha_parent_after, alpha_parent_before,
            "Alpha must keep its OWN parent identity across the swap (not re-homed onto Beta's \
             twin): before {alpha_parent_before}, after {alpha_parent_after}"
        );
        assert_eq!(
            beta_parent_after, beta_parent_before,
            "Beta must keep its OWN parent identity across the swap: before {beta_parent_before}, \
             after {beta_parent_after}"
        );
    });
}

/// RULING A2 (c): a genuinely NEW headline (no same-content candidate) still
/// mints a fresh id, while the pre-existing headline keeps its id. Fresh-mint
/// scope guard — A2 must not over-remap.
#[test]
fn genuinely_new_block_still_mints() {
    let rt = runtime();
    rt.block_on(async {
        const KEEP: &str = "Existing note";
        let one = format!("* {KEEP}\n");

        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("New.org", one.clone())
            .build(rt.clone())
            .await
            .expect("boot");

        let path = env.org_file_path("New.org");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains(":ID:"),
            "first writeback (mint)",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let keep_before = ids_with_content(&env, KEEP).await;
        assert_eq!(keep_before.len(), 1, "one headline ingests as one block");

        // Append a brand-new ID-less headline alongside the (now ID-full) one,
        // then re-write the whole file with BOTH still ID-less (stale re-write).
        let two = format!("* {KEEP}\n* Brand new item\n");
        FileSystem::write(env.org_fs.as_ref(), &path, two.as_bytes())
            .await
            .expect("stale re-write with new block");
        wait_for_disk(
            &env,
            &path,
            |c| c.contains("Brand new item"),
            "second writeback",
        )
        .await;
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

        let keep_after = ids_with_content(&env, KEEP).await;
        assert_eq!(
            keep_after, keep_before,
            "the pre-existing headline keeps its id (before {keep_before:?}, after {keep_after:?})"
        );
        let fresh = ids_with_content(&env, "Brand new item").await;
        assert_eq!(
            fresh.len(),
            1,
            "the genuinely-new headline mints exactly one fresh block"
        );
    });
}
