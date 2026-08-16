//! The native boot path must perform EVERY step in `BootStep::ALL`.
//!
//! Two boot sequences exist in the tree: this one (`add_frontend`, every native
//! frontend) and `holon-worker`'s hand-mirrored `build_engine_state`, which
//! exists only because `holon-app` is not wasm-buildable. Every divergence
//! found so far degraded SILENTLY — a missing `start_action_watchers` showed up
//! as an empty journals feed, a missing CRUD provider as "editor dispatch does
//! nothing". `BootDisclosure` turns each into a startup warning.
//!
//! These tests pin the reference end of that comparison. The report is read off
//! the session itself (`FrontendSession::boot_report`), which exists only
//! because `SessionParts` requires one — an earlier version queried the ledger
//! instead, and could not tell "nothing was skipped" from "the terminator was
//! deleted".
//!
//! The worker's (much larger) skipped set is asserted in `holon-frontend`'s
//! `platform` unit tests, which need no engine.
//!
//! @pbt kind harness
//! @pbt covers native-boot-capability-disclosure — the shared wiring performs
//! every BootStep, so no capability-disabled warning is emitted on native
//! @pbt covers unconfigured-step-config-disclosure — a step the container is
//! not configured for discloses as ConfigAbsent, never as drift, never silently
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone asserts
//! behaviour, never the boot-step ledger

use std::collections::HashSet;
use std::sync::Arc;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_frontend::platform::BootStep;
use holon_frontend::platform::SkipReason;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime"),
    )
}

/// Boot the real shared wiring against a fresh vault and hand back the report.
async fn boot_and_report(dir: &std::path::Path) -> Report {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("disclosure.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, ()) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |_| (),
    )
    .await
    .expect("the shared wiring must boot a session");
    let report = session.boot_report();
    let by_reason = |f: fn(&SkipReason) -> bool| -> Vec<BootStep> {
        report
            .skipped()
            .iter()
            .filter(|s| f(&s.reason))
            .map(|s| s.step)
            .collect()
    };
    Report {
        drift: report.drift(),
        degraded: by_reason(|r| matches!(r, SkipReason::MissingCapability(_))),
        config_absent: by_reason(|r| matches!(r, SkipReason::ConfigAbsent(_))),
    }
}

struct Report {
    drift: Vec<BootStep>,
    degraded: Vec<BootStep>,
    config_absent: Vec<BootStep>,
}

/// A native session must never skip a step for a platform reason, and never
/// drift. (Config-absent steps are legitimate and depend on what is configured
/// — this container has no MCP integration registry, for instance.)
///
/// This is the assertion that goes red when any `performed()` mark is deleted:
/// on native every capability is present, so an unmarked step lands in `drift`.
#[test]
fn native_boot_performs_every_disclosed_step() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir for boot");
        let report = boot_and_report(dir.path()).await;
        assert_eq!(
            report.drift,
            Vec::new(),
            "the native boot path skipped step(s) it is capable of running — each is a feature \
             silently missing from every native frontend"
        );
        assert_eq!(
            report.degraded,
            Vec::new(),
            "native has every capability, so no step may be excused as a platform degradation"
        );
    });
}

/// A step the container is not configured for must be disclosed as config
/// absence — never as drift (which would cry wolf on every plain session) and
/// never silently.
///
/// This container registers no MCP integrations, so the FDW-table pass finds no
/// registry and never runs. Before the ledger distinguished config absence,
/// this step was marked performed unconditionally and the boot reported
/// all-clear.
///
/// Note on coverage: the vault-less variant of this scenario is NOT reachable
/// through `add_frontend`. A session with no vault root cannot boot in either
/// mode — SqlOnly leaves `dyn BlockOrdering` unprovided (no
/// `EventInfraModule`), and Loro-without-vault trips the operation-registry
/// startup check for the missing block-CRUD provider. So `OrgModeIngest`'s
/// config-absent arm is currently only exercised at the unit level
/// (`platform::tests`).
#[test]
fn unconfigured_step_discloses_as_config_absence_not_drift() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir for boot");
        let report = boot_and_report(dir.path()).await;

        assert!(
            report.config_absent.contains(&BootStep::McpFdwTables),
            "the MCP FDW pass found no registry and never ran — it must be disclosed as \
             config-absent; got config_absent={:?} drift={:?}",
            report.config_absent,
            report.drift
        );
        assert!(
            !report.drift.contains(&BootStep::McpFdwTables),
            "a step that is merely unconfigured must not be reported as drift"
        );
    });
}
