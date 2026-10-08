//! The SHIPPED GPUI container must be able to disclose degradation.
//!
//! Symptom (dogfood, BugFunnel 2026-08-04 ENVIRONMENT): booting the real
//! desktop binary logged, under `di.factory.FrontendSession.resolve_engine`,
//!
//! ```text
//! [McpIntegrationsModule] No ConditionBus in this container
//! ((ServiceNotProvided) … integration connect failures will be LOG-ONLY and
//!  their pages will render blank with no banner)
//! ```
//!
//! Root cause: `Arc<ConditionBus>` was registered ONLY by `LoroModule`,
//! which `add_frontend` configures iff `crdt.enabled`. A SqlOnly container
//! therefore had no bus, and every failed MCP integration degraded to an
//! unattributable blank page.
//!
//! This test builds the container the shipped binary builds (`GpuiModule`,
//! the same module `main.rs` hands to fluxdi) across every setting of
//! `crdt.enabled` and asserts the bus resolves. It only
//! runs `configure` — registration is the surface under test, not boot.
//!
//! @pbt kind harness
//! @pbt covers degraded-disclosure-registration — the shipped GPUI DI
//! container provides `Arc<ConditionBus>` in BOTH consolidator modes.
//! Registration only; that a subscriber exists is
//! `degraded_bus_bridge_windowed.rs` (BugFunnel 2026-08-04 ENVIRONMENT)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never
//! assembles the gpui module graph, so it cannot see a missing registration

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Injector;
use fluxdi::Module;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_frontend::config::CrdtPreferences;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::McpConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_gpui::di::GpuiModule;

fn shipped_module(dir: &std::path::Path, crdt_enabled: Option<bool>) -> GpuiModule {
    GpuiModule::new(
        HolonConfig {
            db_path: Some(dir.join("holon.db")),
            vault: VaultConfig {
                root: Some(dir.to_path_buf()),
            },
            crdt: CrdtPreferences {
                enabled: crdt_enabled,
                ..Default::default()
            },
            // No listener in a test; `configure_mcp` is the only thing this
            // flag gates and it is orthogonal to the bus registration.
            mcp: McpConfig {
                enabled: Some(false),
            },
            ..Default::default()
        },
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
    )
}

/// `crdt.enabled = false` — SqlOnly, the configuration the dogfood boot ran in.
#[test]
fn shipped_gpui_container_provides_condition_bus_in_sql_only_mode() {
    assert_bus_resolves(Some(false));
}

/// `crdt.enabled` absent: the resolver picks the shipped default. Asserted as
/// its own arm so the two explicit arms above cannot both stand in for it.
#[test]
fn shipped_gpui_container_provides_condition_bus_on_the_default() {
    assert_bus_resolves(None);
}

/// Loro mode must keep working too — the bus moved out of `LoroModule`, and a
/// double registration or a lost one would show up here.
#[test]
fn shipped_gpui_container_provides_condition_bus_in_loro_mode() {
    assert_bus_resolves(Some(true));
}

fn assert_bus_resolves(crdt_enabled: Option<bool>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let injector = Injector::root();
        shipped_module(dir.path(), crdt_enabled)
            .configure(&injector)
            .expect("GpuiModule::configure (the shipped container assembly)");

        // The exact resolve `McpIntegrationsModule` performs when it decides
        // whether a failed integration can be disclosed.
        injector
            .try_resolve_async::<Arc<ConditionBus>>()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "shipped GPUI container (crdt.enabled = {crdt_enabled:?}) must provide \
                     Arc<ConditionBus> — without it there is no channel on which an \
                     integration connect failure can be disclosed at all: {e}"
                )
            });
    });
}

/// The bus `GpuiModule::new` installs the panic hook on is the bus its
/// container resolves.
#[test]
fn a_panic_reaches_the_bus_the_shipped_container_resolves() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let injector = Injector::root();
        shipped_module(dir.path(), None)
            .configure(&injector)
            .expect("GpuiModule::configure (the shipped container assembly)");
        let bus = injector
            .try_resolve_async::<Arc<ConditionBus>>()
            .await
            .expect("the shipped container's bus");
        assert_a_panic_reaches(&bus, "gpui container bus probe").await;
    });
}

/// The bus a reset installs the panic hook on is the bus the fresh session's
/// injector resolves.
#[test]
fn a_panic_reaches_the_bus_a_reset_session_resolves() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let org_root = dir.path().join("org");
        std::fs::create_dir(&org_root).expect("create the org root");
        let fresh = holon_gpui::reset::build_fresh_sut(
            dir.path().join("holon.db"),
            org_root,
            dir.path().join("config"),
            Duration::ZERO,
        )
        .await
        .expect("build the reset session");
        assert_a_panic_reaches(&fresh.conditions, "gpui reset bus probe").await;
    });
}

async fn assert_a_panic_reaches(bus: &ConditionBus, message: &'static str) {
    let joined = std::thread::spawn(move || panic!("{message}")).join();
    assert!(joined.is_err(), "the probe thread must die of its panic");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !bus.current().iter().any(
        |c| matches!(&c.reason, ConditionKind::TaskPanicked { message: m, .. } if m == message),
    ) {
        assert!(
            Instant::now() < deadline,
            "the panic never reached the session's bus; it has {:?}",
            bus.current()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
