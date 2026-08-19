//! Cold boot must not disclose a shared subtree it merely READ from disk.
//!
//! Symptom (Martin dogfooding a cold boot, 2026-08-18): four yellow banners
//! `Shared edit saved — org file pending`, one per shared subtree in the vault,
//! although nothing had been edited. The count tracked the number of shares,
//! not the number of edits.
//!
//! Root cause: the write-back projection suppresses the disclosure for
//! pre-existing content via a `seeding` flag fed from the `Supervised::Reset`
//! snapshot (`crates/holon-orgmode/src/di.rs`). `Reset` precedes every stream,
//! so on a COLD boot it fires before the initial scan has ingested anything and
//! the snapshot is empty. Every block the scan then creates FROM disk arrived
//! as an ordinary post-snapshot upsert and disclosed — announcing an unsaved
//! edit for content that had just been read off the disk it was allegedly
//! missing from. Warm boots were unaffected, which is why this only ever showed
//! cold.
//!
//! Fix: an id-set attempt was MEASURED flaky — boot re-projects a block more
//! than once, so any suppression set with a bounded lifetime loses the race.
//! The disclosure therefore MOVED to the write-back path
//! (`FileSyncController::disclose_share_inlined_into`). It fires on a real
//! write attempt, which cold-boot ingest never makes — reading a file produces
//! no write — and it names the FILE the shared content was inlined into.
//!
//! BugFunnel 2026-08-18-cold-boot-discloses-shared-edit-for-every-share.
//!
//! @pbt kind harness
//! @pbt covers cold-boot-share-disclosure — a cold boot over a vault holding a
//! shared subtree raises no SharedSubtreeNotMaterialized condition
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone catalog cannot
//! generate a share at all (no share-role / shared-tree-id anywhere in
//! crates/holon-integration-tests/src), and the disclosure seam is wired only
//! in crates/holon-app/src/wiring.rs

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
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

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

const SHARE_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A shared subtree in the shape a real vault holds one: the mount is a
/// HEADLINE carrying `:share-role: mount` (not a page), so the owning-page walk
/// terminates at a non-mount page and the not-materialized condition genuinely
/// holds. That is what makes this fixture load-bearing — the disclosure is
/// suppressed here because nothing was EDITED, not because the share looks
/// materialized.
const SHARED_FILE: &str = "\
* Shared tree (11111111-2222-3333-4444-555555555555)
:PROPERTIES:
:ID: share-mount-block
:share-role: mount
:shared-tree-id: 11111111-2222-3333-4444-555555555555
:END:
** First shared child
:PROPERTIES:
:ID: share-child-one
:shared-tree-id: 11111111-2222-3333-4444-555555555555
:END:
** Second shared child
:PROPERTIES:
:ID: share-child-two
:shared-tree-id: 11111111-2222-3333-4444-555555555555
:END:
";

const PLAIN_FILE: &str = "\
* Plain Root
:PROPERTIES:
:ID: plain-root
:END:
";

/// Every `SharedSubtreeNotMaterialized` condition currently raised on the bus.
/// The bus is sticky by design (`degraded_signal_bus.rs`: "a condition raised
/// during boot DI still reaches a window that launches later"), so subscribing
/// after boot sees whatever the boot raised.
fn not_materialized_subjects(env: &holon_integration_tests::TestEnvironment) -> Vec<String> {
    let injector = env
        .injector()
        .expect("test environment must expose its injector");
    let bus = injector.resolve::<Arc<holon_loro::DegradedSignalBus>>();
    bus.subscribe()
        .current
        .into_iter()
        .filter(|c| {
            matches!(
                c.reason,
                holon_loro::ShareDegradedReason::SharedSubtreeNotMaterialized { .. }
            )
        })
        .map(|c| c.shared_tree_id)
        .collect()
}

/// THE REGRESSION. A cold boot over a vault containing a shared subtree must
/// raise NO not-materialized condition: the scan READ that content from disk,
/// so no edit is pending against it.
#[test]
fn cold_boot_over_a_shared_subtree_discloses_nothing() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("shared.org", SHARED_FILE)
            .with_org_file("plain.org", PLAIN_FILE)
            .build(rt.clone())
            .await
            .expect("cold boot over a vault with a shared subtree must succeed");

        // NOT VACUOUS: the scan really did ingest the shared blocks. Without
        // this the assertion below would also pass on a boot that scanned
        // nothing at all.
        for id in ["share-mount-block", "share-child-one", "share-child-two"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "shared block {id} did not ingest — the fixture never reached the projection, so \
                 this test proves nothing"
            );
        }

        let disclosed = not_materialized_subjects(&env);
        assert!(
            disclosed.is_empty(),
            "a cold boot disclosed a shared subtree it only READ from disk: {disclosed:?} — the \
             ingest makes no write attempt, so nothing should have been disclosed"
        );
    });
}

/// TEETH. The fix must not have simply switched the disclosure off: a genuine
/// STORE-side edit to a share whose mount is not a page still discloses.
///
/// The edit has to originate in the store, not in the file. An edit authored by
/// writing the org file is already ON disk, so it has no write-back gap to
/// disclose — that asymmetry is the whole point of sourcing the disclosure from
/// a real write attempt.
#[test]
fn a_post_boot_edit_to_an_unmaterialized_share_still_discloses() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("shared.org", SHARED_FILE)
            .build(rt.clone())
            .await
            .expect("cold boot over a vault with a shared subtree must succeed");

        for id in ["share-mount-block", "share-child-one"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "shared block {id} did not ingest"
            );
        }
        assert!(
            not_materialized_subjects(&env).is_empty(),
            "boot must be quiet before the edit, else this test cannot attribute the disclosure \
             to the edit"
        );

        // A real edit, made in the STORE: a new block under the share's mount.
        // The projection must now carry it to disk, and the share owns no file
        // of its own to carry it to.
        env.create_block(
            "block:share-child-three",
            "block:share-mount-block",
            "third shared child",
        )
        .await
        .expect("store-side create inside the shared subtree");

        // Poll: the disclosure lands when the write-back reaches the file, which
        // is a step behind the store write.
        let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
        let mut disclosed = not_materialized_subjects(&env);
        while disclosed.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            disclosed = not_materialized_subjects(&env);
        }
        assert_eq!(
            disclosed,
            vec![SHARE_ID.to_string()],
            "a genuine store-side edit to a share with no page-mount MUST still disclose — \
             suppressing it would hide a real write-back gap"
        );
    });
}

/// WARM BOOT. Restarting over a vault whose blocks the store ALREADY holds must
/// stay silent too.
///
/// The warm path is the one that always worked under the old feed-side
/// predicate — the `Reset` snapshot covered it — so it is the case a regression
/// would silently take back. Nothing was edited across the restart, so no write
/// is owed and no banner is due.
#[test]
fn a_warm_restart_over_the_same_vault_discloses_nothing() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let mut harness = TestEnvironmentBuilder::new()
            .with_org_file("shared.org", SHARED_FILE)
            .build(rt.clone())
            .await
            .expect("cold boot over a vault with a shared subtree must succeed");

        for id in ["share-mount-block", "share-child-one"] {
            assert!(
                harness
                    .wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "shared block {id} did not ingest on the first boot"
            );
        }
        assert!(
            not_materialized_subjects(&harness).is_empty(),
            "the first boot must be silent before the restart is meaningful"
        );

        harness.stop_app().await.expect("stop the app");
        harness
            .start_app(true)
            .await
            .expect("restart over the same vault");

        assert!(
            harness
                .wait_for_block("block:share-child-one", SYNC_TIMEOUT)
                .await,
            "the shared blocks must still be present after the restart"
        );
        let disclosed = not_materialized_subjects(&harness);
        assert!(
            disclosed.is_empty(),
            "a warm restart disclosed a share nobody edited: {disclosed:?}"
        );
    });
}
