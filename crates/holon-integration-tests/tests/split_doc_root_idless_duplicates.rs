//! Regression pin: a file whose declared `#+ID:` doc anchor is DISJOINT from
//! the subtree its blocks actually live in must be QUARANTINED, not minted
//! into. Before the guard it re-minted every ID-less headline on EVERY ingest,
//! accreting duplicates without bound.
//!
//! Mechanism. `CacheBlockReader::get_blocks(doc_id)` walks a recursive CTE
//! down from `parent_id = doc_id` and excludes `Page`-tagged blocks together
//! with their subtrees. When the anchor's only child is itself a page, that
//! read is EXACTLY EMPTY. The ID-less reconcile in
//! `FileSyncController::ingest_file` uses it as `existing`, and in
//! `tiered_match` an empty candidate set is the sole `MintFresh` path — so
//! every ID-less headline mints a new UUID each pass. The created blocks are
//! parented by the `:ID:` written in the FILE, which sits in the OTHER root's
//! subtree, so the copies accrete there while the candidate read keeps staring
//! at the empty anchor. Nothing prunes them: the diff base is that same empty
//! read, so the delete pass has nothing to diff against, and the write-back
//! renders from the empty anchor and never stamps an `:ID:` back — leaving the
//! headline ID-less on disk for the next pass to mint again.
//!
//! Contract under test: `assert_mint_parents_inside_doc_anchor` refuses the
//! ingest, the `Err` quarantines the file from write-back, and the refusal
//! discloses the anchor, the offending authored block and its real owner.
//! REPAIR semantics — reconciling the ID-less headlines onto the authored
//! parent's subtree so they ingest normally across a split root — is a
//! deliberately out-of-scope alternative pending a ruling by Martin.
//!
//! Shape under test, mirroring the live vault's
//! `Projects/Holon/Prepare personal usage.org`:
//! - `Projects/Holon.org` holds an untagged chain `Tasks` -> `Prepare personal
//!   usage` -> `Fix bugs`, so `Fix bugs` is authoritatively owned by that
//!   document.
//! - `Projects/Holon/Prepare personal usage.org` declares a DIFFERENT `#+ID:` —
//!   a `Page`-tagged sibling under `Holon` — and carries the same `Fix bugs`
//!   `:ID:` as its top-level headline. The cross-doc-membership guard refuses
//!   to adopt that block, so the anchor never gains a non-page child.
//! - `Projects/Holon/Prepare personal usage/DeleteMe.org` makes the anchor's
//!   sole child a page, mirroring the live `DeleteMe` block.
//!
//! The split root is established by letting the page-file APPEAR after the
//! chain is already in the store — the order the live vault reached this state
//! — rather than by hand-writing store rows.
//!
//! @pbt kind harness
//! @pbt covers split-doc-root-idless-duplicates — an ID-less headline under a
//! doc anchor disjoint from its content subtree must be quarantined, never
//! minted (dogfood 2026-07-28, BugFunnel)

use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;

/// Runtime whose workers are bound to this case's observability scope, so the
/// quarantine `error!` — emitted on a tokio worker, not the test thread — is
/// attributed here and readable through `captured_problems`.
fn runtime() -> Arc<tokio::runtime::Runtime> {
    // Installs the subscriber carrying `ErrorCaptureLayer`. Must happen before
    // the SUT runs, or the refusal is emitted with no layer to record it.
    SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    Arc::new(builder.build().expect("Failed to create runtime"))
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_millis(500);

const HOLON: &str = "11111111-1111-1111-1111-111111111111";
const TASKS: &str = "22222222-2222-2222-2222-222222222222";
/// The UNTAGGED "Prepare personal usage" headline that owns the real content.
const PPU_INLINE: &str = "33333333-3333-3333-3333-333333333333";
/// The headline both files claim; it stays under `PPU_INLINE`.
const FIXBUGS: &str = "44444444-4444-4444-4444-444444444444";
/// The page-file's declared `#+ID:` anchor — a `Page` sibling under `Holon`.
const PPU_PAGE: &str = "55555555-5555-5555-5555-555555555555";
/// The anchor's sole child, itself a page, which empties the CTE read.
const DELME: &str = "66666666-6666-6666-6666-666666666666";
/// A well-formed page whose anchor DOES own its content (the healthy control).
const HEALTHY: &str = "77777777-7777-7777-7777-777777777777";

const PROBE: &str = "Dogfood duplication probe";
const HEALTHY_PROBE: &str = "Healthy control headline";

const CHAIN_PATH: &str = "Projects/Holon.org";
const PAGE_PATH: &str = "Projects/Holon/Prepare personal usage.org";
const DELME_PATH: &str = "Projects/Holon/Prepare personal usage/DeleteMe.org";
const HEALTHY_PATH: &str = "Projects/Healthy.org";

fn chain_org() -> String {
    format!(
        "#+ID: {HOLON}\n#+TITLE: Holon\n\n\
         * Tasks\n:PROPERTIES:\n:ID: {TASKS}\n:END:\n\
         ** Prepare personal usage\n:PROPERTIES:\n:ID: {PPU_INLINE}\n:END:\n\
         *** Fix bugs\n:PROPERTIES:\n:ID: {FIXBUGS}\n:END:\n"
    )
}

fn page_org(extra: &str) -> String {
    format!(
        "#+ID: {PPU_PAGE}\n#+TITLE: Prepare personal usage\n\n\
         * Fix bugs\n:PROPERTIES:\n:ID: {FIXBUGS}\n:END:\n{extra}"
    )
}

fn delme_org() -> String {
    format!("#+ID: {DELME}\n#+TITLE: DeleteMe\n")
}

fn healthy_org() -> String {
    format!("#+ID: {HEALTHY}\n#+TITLE: Healthy\n\n* {HEALTHY_PROBE}\n")
}

/// All store block ids whose `content` equals `content` exactly, sorted.
async fn ids_with_content(env: &TestEnvironment, content: &str) -> Vec<String> {
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

/// The `parent_id` of the block with this exact id. Panics if absent.
async fn parent_of_id(env: &TestEnvironment, id: &str) -> String {
    let rows = env
        .query(
            &format!("from block | filter id == \"block:{id}\" | select {{id, parent_id}}"),
            QueryLanguage::HolonPrql,
        )
        .await
        .expect("parent query failed");
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one block {id}, got {rows:?}"
    );
    rows[0]
        .get("parent_id")
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("block {id} has no parent_id: {rows:?}"))
}

async fn page_tagged(env: &TestEnvironment, id: &str) -> bool {
    let rows = env
        .query(
            &format!(
                "from block_tags | filter block_id == \"block:{id}\" | filter tag == \"Page\" | \
                 select {{block_id, tag}}"
            ),
            QueryLanguage::HolonPrql,
        )
        .await
        .expect("tag query failed");
    !rows.is_empty()
}

/// Every ERROR-level event captured for this case, joined — the quarantine
/// refusal reaches here through `on_file_changed`'s `error!` disclosure.
fn captured_errors() -> String {
    SpanCollector::global()
        .captured_problems()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// How many split-doc-root refusals have been captured so far.
fn split_root_refusals() -> usize {
    SpanCollector::global()
        .captured_problems()
        .iter()
        .filter(|p| p.to_string().contains("split doc root"))
        .count()
}

/// Boot the chain, then let the page-file appear, and assert the split root
/// actually formed — so a harness break is distinguishable from the bug.
async fn split_root_env(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let env = TestEnvironmentBuilder::new()
        .without_loro()
        .with_org_file(CHAIN_PATH, chain_org())
        .build(rt)
        .await
        .expect("boot");
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

    let page_path = env.org_file_path(PAGE_PATH);
    let delme_path = env.org_file_path(DELME_PATH);
    env.org_fs.mkdir_all(
        delme_path
            .parent()
            .expect("DeleteMe.org path has a parent dir"),
    );
    FileSystem::write(env.org_fs.as_ref(), &delme_path, delme_org().as_bytes())
        .await
        .expect("write DeleteMe.org");
    FileSystem::write(env.org_fs.as_ref(), &page_path, page_org("").as_bytes())
        .await
        .expect("write page file");
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
    tokio::time::sleep(SETTLE).await;

    assert!(
        page_tagged(&env, PPU_PAGE).await,
        "precondition: the page-file's declared anchor must be Page-tagged"
    );
    assert!(
        page_tagged(&env, DELME).await,
        "precondition: the anchor's sole child must be Page-tagged (it is what \
         empties the CTE read)"
    );
    assert_eq!(
        parent_of_id(&env, DELME).await,
        format!("block:{PPU_PAGE}"),
        "precondition: DeleteMe must be the anchor's child"
    );
    assert_eq!(
        parent_of_id(&env, FIXBUGS).await,
        format!("block:{PPU_INLINE}"),
        "precondition: the content subtree must stay under the untagged chain, \
         disjoint from the declared anchor"
    );
    env
}

/// Write the page-file with an ID-less headline under `Fix bugs`, let it
/// ingest, settle. Returns the exact bytes written, so the caller can assert
/// the write-back was skipped.
async fn write_idless(env: &TestEnvironment, label: &str) -> String {
    let page_path = env.org_file_path(PAGE_PATH);
    let body = page_org(&format!("** {PROBE}\n"));
    FileSystem::write(env.org_fs.as_ref(), &page_path, body.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("write page file [{label}]: {e}"));
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
    tokio::time::sleep(SETTLE).await;
    body
}

/// The core contract: the ingest is REFUSED, nothing is minted, the refusal
/// names the anchor, the offending authored block and its real owner, and the
/// file on disk is left byte-identical (write-back quarantined).
#[test]
fn split_doc_root_is_quarantined_never_minted() {
    let rt = runtime();
    rt.block_on(async {
        let env = split_root_env(rt.clone()).await;

        let written = write_idless(&env, "quarantined ingest").await;

        let minted = ids_with_content(&env, PROBE).await;
        assert!(
            minted.is_empty(),
            "a split doc root must mint NOTHING, got {minted:?}"
        );

        let errors = captured_errors();
        assert!(
            errors.contains("split doc root"),
            "the refusal must be disclosed as a quarantine, captured errors:\n{errors}"
        );
        for (what, id) in [
            ("the declared anchor", PPU_PAGE),
            ("the offending authored block", FIXBUGS),
            ("the anchor that really owns it", HOLON),
        ] {
            assert!(
                errors.contains(id),
                "the refusal must name {what} ({id}), captured errors:\n{errors}"
            );
        }

        let on_disk = env
            .org_fs
            .read_to_string(&env.org_file_path(PAGE_PATH))
            .await
            .expect("read page file");
        assert_eq!(
            on_disk, written,
            "a quarantined file must not be written back over"
        );
    });
}

/// The accretion facet, inverted: repeated ingests of the same refused file
/// stay at zero blocks instead of adding a copy per pass. Each round must
/// capture a FRESH refusal — zero blocks alone would also hold if the file had
/// simply stopped being ingested at all.
#[test]
fn quarantine_is_stable_across_reingests() {
    let rt = runtime();
    rt.block_on(async {
        let env = split_root_env(rt.clone()).await;

        for label in ["ingest 1", "ingest 2", "ingest 3"] {
            let refusals_before = split_root_refusals();
            let written = write_idless(&env, label).await;
            let refusals_after = split_root_refusals();
            assert!(
                refusals_after > refusals_before,
                "the guard must refuse AGAIN at [{label}], not merely leave the \
                 file un-ingested (refusals {refusals_before} -> {refusals_after})"
            );
            let minted = ids_with_content(&env, PROBE).await;
            assert!(
                minted.is_empty(),
                "still nothing may be minted at [{label}], got {minted:?}"
            );
            let on_disk = env
                .org_fs
                .read_to_string(&env.org_file_path(PAGE_PATH))
                .await
                .expect("read page file");
            assert_eq!(
                on_disk, written,
                "the file must stay untouched at [{label}]"
            );
        }
    });
}

/// The discriminating control: a well-formed file — whose anchor owns its own
/// content — still mints its ID-less headline exactly once and reconciles on
/// re-ingest. The guard must fire on the split root ONLY.
#[test]
fn healthy_doc_root_still_mints_once() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file(HEALTHY_PATH, healthy_org())
            .build(rt.clone())
            .await
            .expect("boot");
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
        tokio::time::sleep(SETTLE).await;

        let first = ids_with_content(&env, HEALTHY_PROBE).await;
        assert_eq!(
            first.len(),
            1,
            "a healthy anchor must mint its ID-less headline exactly once, got {first:?}"
        );

        // Re-write the same pre-mint (still ID-less) text: the reconcile must
        // land on the existing block, not on the guard.
        FileSystem::write(
            env.org_fs.as_ref(),
            &env.org_file_path(HEALTHY_PATH),
            healthy_org().as_bytes(),
        )
        .await
        .expect("re-write healthy file");
        env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;
        tokio::time::sleep(SETTLE).await;

        let second = ids_with_content(&env, HEALTHY_PROBE).await;
        assert_eq!(
            second, first,
            "a healthy re-ingest must reconcile, not mint or quarantine \
             (before {first:?}, after {second:?})"
        );

        let errors = captured_errors();
        assert!(
            !errors.contains("split doc root"),
            "the guard must NOT fire on a well-formed file, captured errors:\n{errors}"
        );
    });
}
