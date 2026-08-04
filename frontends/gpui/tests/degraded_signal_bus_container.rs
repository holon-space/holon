//! The SHIPPED GPUI container must be able to disclose degradation.
//!
//! Symptom (dogfood, BugFunnel 2026-08-04 ENVIRONMENT): booting the real
//! desktop binary logged, under `di.factory.FrontendSession.resolve_engine`,
//!
//! ```text
//! [McpIntegrationsModule] No DegradedSignalBus in this container
//! ((ServiceNotProvided) … integration connect failures will be LOG-ONLY and
//!  their pages will render blank with no banner)
//! ```
//!
//! Root cause: `Arc<DegradedSignalBus>` was registered ONLY by `LoroModule`,
//! which `add_frontend` configures iff `crdt.enabled` — and that defaults to
//! `false`. So the shipped default container had no bus, and every failed MCP
//! integration degraded to an unattributable blank page.
//!
//! This test builds the container the shipped binary builds (`GpuiModule`,
//! the same module `main.rs` hands to fluxdi) with the SHIPPED DEFAULTS for
//! `crdt.enabled` (absent → SqlOnly) and asserts the bus resolves. It only
//! runs `configure` — registration is the surface under test, not boot.
//!
//! @pbt kind harness
//! @pbt covers degraded-disclosure-registration — the shipped GPUI DI
//! container provides `Arc<DegradedSignalBus>` in BOTH consolidator modes.
//! Registration only; that a subscriber exists is
//! `degraded_bus_bridge_windowed.rs` (BugFunnel 2026-08-04 ENVIRONMENT)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never
//! assembles the gpui module graph, so it cannot see a missing registration

use std::collections::HashSet;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use holon::sync::DegradedSignalBus;
use holon_frontend::config::CrdtPreferences;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::McpConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_gpui::di::GpuiModule;

fn shipped_module(dir: &std::path::Path, crdt_enabled: Option<bool>) -> GpuiModule {
    GpuiModule {
        holon_config: HolonConfig {
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
                ..Default::default()
            },
            ..Default::default()
        },
        session_config: SessionConfig::new(holon_api::UiInfo::permissive()),
        config_dir: dir.to_path_buf(),
        locked_keys: HashSet::new(),
    }
}

/// `crdt.enabled` absent — the SHIPPED default, and the configuration the
/// dogfood boot ran in.
#[test]
fn shipped_gpui_container_provides_degraded_signal_bus_in_sql_only_mode() {
    assert_bus_resolves(None);
}

/// Loro mode must keep working too — the bus moved out of `LoroModule`, and a
/// double registration or a lost one would show up here.
#[test]
fn shipped_gpui_container_provides_degraded_signal_bus_in_loro_mode() {
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
            .try_resolve_async::<Arc<DegradedSignalBus>>()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "shipped GPUI container (crdt.enabled = {crdt_enabled:?}) must provide \
                     Arc<DegradedSignalBus> — without it there is no channel on which an \
                     integration connect failure can be disclosed at all: {e}"
                )
            });
    });
}
