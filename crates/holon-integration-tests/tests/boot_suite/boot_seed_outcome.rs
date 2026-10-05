//! The boot seed of the default layout runs after the org initial scan
//! (D83.a). Its outcome must reach the boot report, and a controller that
//! dies before it reports the scan must not park the seed forever.
//!
//! @pbt kind harness
//! @pbt covers boot-seed-outcome-disclosure — the boot report records the
//! default-layout seed as performed only after it succeeded
//! @pbt covers boot-seed-controller-death — a controller that dies during the
//! initial scan fails the boot loud instead of parking the seed
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone asserts
//! behaviour, never the boot-step ledger, and injects no controller death

use std::sync::Arc;
use std::time::Duration;

use holon_frontend::platform::BootStep;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const NOTES_ORG: &str = "\
#+TITLE: Notes
#+ID: notes
* Note one
:PROPERTIES:
:ID: notes-1
:END:
";

/// A scan that could not run skips the seed, and the report must say so
/// instead of claiming the seed was performed.
#[test]
fn a_skipped_seed_is_reported_as_failed_not_performed() {
    let rt = runtime();
    rt.clone().block_on(async {
        holon_filesystem::crash_injection::arm("scan_vault_files");
        let env = TestEnvironmentBuilder::new()
            .with_org_file("notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault walk that fails is disclosed, not a boot failure");
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: the boot never walked the vault"
        );

        let report = env.session().boot_report();
        let failed: Vec<BootStep> = report.failed().into_iter().map(|(s, _)| s).collect();
        assert!(
            failed.contains(&BootStep::SeedDefaultLayout),
            "the seed was skipped, so the report must record it as failed; failed={failed:?} \
             skipped={:?} in_background={:?}",
            report.skipped(),
            report.in_background()
        );
    });
}

/// Production does not wait for the seed: the report closes while it runs and
/// records it as performed once it succeeds.
#[test]
fn a_background_seed_is_reported_as_performed_once_it_succeeds() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("notes.org", NOTES_ORG)
            .wait_for_file_watcher(false)
            .build(rt.clone())
            .await
            .expect("boot");

        let report = env.session().boot_report();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while report
            .in_background()
            .contains(&BootStep::SeedDefaultLayout)
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the background seed did not end within 30 s"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(report.failed(), Vec::new());
        assert!(
            !report
                .skipped()
                .iter()
                .any(|s| s.step == BootStep::SeedDefaultLayout),
            "the seed ran, so it must not be reported as skipped: {:?}",
            report.skipped()
        );
        assert!(
            env.wait_for_block("block:root-layout", Duration::from_secs(10))
                .await,
            "the report says performed, so the layout must be in the store"
        );
    });
}

/// The controller panics inside the initial-scan ingest, before it reports the
/// scan to the seed. The boot must end and report the seed as failed instead
/// of parking it.
#[test]
fn a_controller_that_dies_during_the_scan_does_not_park_the_seed() {
    let rt = runtime();
    rt.clone().block_on(async {
        holon_filesystem::crash_injection::arm("initial_scan_ingest");
        let rt_for_boot = rt.clone();
        let boot = tokio::spawn(async move {
            let env = TestEnvironmentBuilder::new()
                .with_org_file("notes.org", NOTES_ORG)
                .build(rt_for_boot)
                .await
                .expect("a dead controller is disclosed, not a boot failure");
            env.session().boot_report().failed()
        });
        let failed = tokio::time::timeout(Duration::from_secs(60), boot)
            .await
            .expect(
                "the boot parked: the seed waits for a scan report a dead controller never sends",
            )
            .expect("boot task");
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: the controller reached the initial-scan ingest"
        );
        let (_, why) = failed
            .iter()
            .find(|(step, _)| *step == BootStep::SeedDefaultLayout)
            .unwrap_or_else(|| panic!("the seed must be reported as failed; failed={failed:?}"));
        assert!(
            why.contains("ended before it reported the initial scan"),
            "the failure must name the dead controller; got: {why}"
        );
    });
}
