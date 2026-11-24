//! A boot step that fails from data or the environment degrades the session
//! instead of aborting the boot: the report names the step and its error, and
//! the session still renders, writes and syncs.
//!
//! @pbt kind harness
//! @pbt covers boot-step-failure-disclosure — one failed boot step boots Ok,
//! the report lists exactly that step with its error, and no other step fails
//! @pbt covers boot-step-failure-usable-session — after a failed boot step the
//! session renders the root, creates a block and settles without a panic
//! @pbt covers vault-watcher-failure-keeps-file-sync — an OS that refuses the
//! file watcher boots Ok with one disclosure naming the cause, and the vault is
//! still ingested and written back
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone asserts
//! behaviour, never the boot-step ledger, and draws no boot step faults

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use holon_frontend::platform::BootStep;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_integration_tests::pbt::composed::boot_fault::boot_step_fault_error;
use holon_pbt_core::BootStepFault;
use holon_pbt_core::any_boot_step_fault;
use proptest::prelude::*;

const NOTES_ORG: &str = "\
#+TITLE: Notes
#+ID: notes
* Note one
:PROPERTIES:
:ID: notes-1
:END:
";

const BOOT_DEADLINE: Duration = Duration::from_secs(120);
const BACKGROUND_DEADLINE: Duration = Duration::from_secs(30);

/// Where a fault's error must show up.
enum Disclosure {
    /// The boot report lists this step, and only it, as failed.
    FailedStep(BootStep),
    /// No step fails; the file sync runs on and one condition says the vault
    /// is not watched.
    VaultNotWatched,
}

fn expected_disclosure(fault: BootStepFault) -> Disclosure {
    match fault {
        BootStepFault::IntegrationProjection => {
            Disclosure::FailedStep(BootStep::IntegrationStateProjection)
        }
        BootStepFault::ActionWatchers => Disclosure::FailedStep(BootStep::StartActionWatchers),
        BootStepFault::LayoutSeed => Disclosure::FailedStep(BootStep::SeedDefaultLayout),
        BootStepFault::SyncController => Disclosure::FailedStep(BootStep::PostReady),
        BootStepFault::IntegrationSupervisor => Disclosure::FailedStep(BootStep::McpFdwTables),
        BootStepFault::VaultWatcher | BootStepFault::VaultWatchRegistration => {
            Disclosure::VaultNotWatched
        }
    }
}

fn vault_not_watched_causes(session: &TestEnvironment) -> Vec<String> {
    session
        .injector()
        .expect("a built session has an injector")
        .resolve::<Arc<holon_conditions::ConditionBus>>()
        .current()
        .into_iter()
        .filter_map(|c| match c.reason {
            holon_conditions::ConditionKind::VaultSyncNotStarted { cause } => Some(cause),
            _ => None,
        })
        .collect()
}

/// A SqlOnly session has no sync controller to fail, so that fault always
/// boots with Loro.
fn with_loro(fault: BootStepFault, loro: bool) -> bool {
    loro || fault == BootStepFault::SyncController
}

async fn boot_and_use(
    rt: Arc<tokio::runtime::Runtime>,
    fault: BootStepFault,
    loro: bool,
) -> Result<(), String> {
    let builder = TestEnvironmentBuilder::new()
        .with_org_file("notes.org", NOTES_ORG)
        .with_fake_mcp()
        .with_boot_step_fault(fault);
    let builder = if loro {
        builder
    } else {
        builder.without_loro()
    };
    let env = tokio::time::timeout(BOOT_DEADLINE, builder.build(rt))
        .await
        .map_err(|_| format!("the boot did not end within {BOOT_DEADLINE:?}"))?
        .map_err(|e| format!("the boot failed instead of degrading: {e:#}"))?;

    let report = env.session().boot_report();
    let deadline = tokio::time::Instant::now() + BACKGROUND_DEADLINE;
    while !report.in_background().is_empty() {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "background steps did not end within {BACKGROUND_DEADLINE:?}: {:?}",
                report.in_background()
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let failed = report.failed();
    let injected = boot_step_fault_error(fault);
    let disclosure = expected_disclosure(fault);
    match &disclosure {
        Disclosure::FailedStep(expected) => match failed.as_slice() {
            [(step, why)] if step == expected && why.contains(&injected) => {}
            _ => {
                return Err(format!(
                    "the report must list exactly `{expected}` failed with `{injected}`; \
                     failed={failed:?} skipped={:?}",
                    report.skipped()
                ));
            }
        },
        Disclosure::VaultNotWatched => {
            if !failed.is_empty() {
                return Err(format!("no boot step may fail; failed={failed:?}"));
            }
            holon_integration_tests::polling::wait_until(
                "the vault-not-watched condition raised",
                BACKGROUND_DEADLINE,
                || async {
                    if vault_not_watched_causes(&env).is_empty() {
                        holon_integration_tests::polling::Probe::NotYet("none".into())
                    } else {
                        holon_integration_tests::polling::Probe::Reached(())
                    }
                },
            )
            .await
            .map_err(|hung| hung.to_string())?;
            match vault_not_watched_causes(&env).as_slice() {
                [cause] if cause.contains(&injected) => {}
                causes => {
                    return Err(format!(
                        "exactly one vault-not-watched condition must name `{injected}`; \
                         causes={causes:?}"
                    ));
                }
            }
        }
    }
    if let Some(point) = holon_filesystem::crash_injection::armed() {
        return Err(format!("premise: the boot never reached `{point}`"));
    }

    match env.initial_widget().await {
        Ok(_) => {}
        Err(e) if fault == BootStepFault::LayoutSeed => {
            tracing::info!("no root layout after a failed seed: {e:#}");
        }
        Err(e) => return Err(format!("rendering the root failed: {e:#}")),
    }
    if let Disclosure::VaultNotWatched = disclosure {
        let ingested = env
            .query_sql("SELECT id FROM block_raw WHERE id = 'block:notes-1'")
            .await
            .map_err(|e| format!("reading the ingested block failed: {e:#}"))?;
        if ingested.len() != 1 {
            return Err(format!("the vault was not ingested: {ingested:?}"));
        }
    }
    env.create_block("boot-fault-probe", "notes", "probe")
        .await
        .map_err(|e| format!("creating a block failed: {e:#}"))?;
    if let Disclosure::VaultNotWatched = disclosure {
        holon_integration_tests::polling::wait_for_file_condition(
            env.org_fs.as_ref(),
            &env.org_root().join("notes.org"),
            |content| content.contains("boot-fault-probe"),
            BACKGROUND_DEADLINE,
        )
        .await
        .map_err(|hung| format!("the new block was not written back: {hung}"))?;
    }
    env.settle().await;
    Ok(())
}

fn check(fault: BootStepFault, loro: bool) -> Result<(), TestCaseError> {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime"),
    );
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        rt.block_on(boot_and_use(rt.clone(), fault, loro))
    }));
    holon_filesystem::crash_injection::disarm();
    match outcome {
        Ok(result) => result.map_err(TestCaseError::fail),
        Err(panic) => Err(TestCaseError::fail(format!(
            "panicked: {}",
            panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic>")
        ))),
    }
}

#[test]
fn a_failed_boot_step_degrades_the_session_and_is_reported() {
    let mut failures: Vec<String> = BootStepFault::ALL
        .into_iter()
        .flat_map(|fault| [(fault, true), (fault, with_loro(fault, false))])
        .filter_map(|(fault, loro)| {
            check(fault, loro)
                .err()
                .map(|e| format!("{fault:?} loro={loro}: {e}"))
        })
        .collect();
    if let Err(e) = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 4,
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(&(any_boot_step_fault(), any::<bool>()), |(fault, loro)| {
        check(fault, with_loro(fault, loro))
    }) {
        failures.push(format!("generated: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
