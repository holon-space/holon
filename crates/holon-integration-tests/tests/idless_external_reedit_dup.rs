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
//! by exact CONTENT + sibling POSITION (`FileSyncController::ingest_file` →
//! `compute_idless_remaps`), so the stale re-write reconciles onto the existing
//! block — its id stays STABLE and it is never duplicated. Matching is
//! positional 1:1, so two genuinely-distinct ID-less siblings with identical
//! content stay two blocks.
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
