//! FluxDI module for the GPUI frontend.
//!
//! `GpuiModule` composes [`CoreInfraModule`] and [`HolonFrontendModule`] via
//! explicit delegation (not `imports()`, which creates child injector scopes
//! that can't see sibling registrations), then adds GPUI-specific services.
//!
//! Lifecycle:
//! - `configure()`: core infra → frontend services → render interpreter → MCP
//!   server
//! - `on_start()`: schema init → session resolution → slot population → MCP
//!   start
//! - `on_stop()`: graceful MCP server shutdown

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::ModuleLifecycleFuture;
use fluxdi::Shared;
use holon::di::CoreInfraModule;
use holon_app::HolonFrontendModule;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::BuilderServicesSlot;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_mcp::McpInjectorExt;
use holon_mcp::di::McpServerHandle;

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("GpuiModule", phase, &e.to_string())
}

pub struct GpuiModule {
    pub holon_config: HolonConfig,
    pub session_config: SessionConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<PrefKey>,
}

impl GpuiModule {
    fn core_module(&self) -> CoreInfraModule {
        CoreInfraModule {
            db_path: self.holon_config.resolve_db_path(&self.config_dir),
        }
    }

    fn frontend_module(&self) -> HolonFrontendModule {
        HolonFrontendModule {
            holon_config: self.holon_config.clone(),
            session_config: self.session_config.clone(),
            config_dir: self.config_dir.clone(),
            locked_keys: self.locked_keys.clone(),
        }
    }

    /// The single MCP composition seam: register the embedded MCP server iff
    /// `mcp.enabled` (default `true`). When disabled, NOTHING is registered —
    /// so there is no `McpServerHandle` to resolve, `on_start` spawns no
    /// task, and nothing binds the loopback listener (attack-surface
    /// reduction). The disabled path is disclosed with one boot log line —
    /// a silent absence would violate fail-loud.
    ///
    /// Gating lives here (and the symmetric `mcp_enabled()` guards in
    /// `on_start` / `on_stop`), NOT scattered inside individual MCP tools: this
    /// is boot-time on/off of the whole server, not per-tool permissions.
    fn configure_mcp(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        if !self.holon_config.mcp_enabled() {
            tracing::info!(
                "MCP server disabled by config (mcp.enabled = false) — no MCP server, no \
                 listener, no MCP task"
            );
            return Ok(());
        }

        let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
            .ok() // ALLOW(ok): non-critical env var
            .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
            .unwrap_or(8520);
        injector.add_mcp_server(mcp_port)?;
        Ok(())
    }
}

impl Module for GpuiModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        self.core_module().configure(injector)?;
        self.frontend_module().configure(injector)?;

        // GPUI-specific: render interpreter + debug services
        let slot = injector.resolve::<BuilderServicesSlot>();
        injector.set_render_interpreter(crate::make_interpret_fn(slot.0.clone()));
        // `register_debug_services` stays unconditional: `DebugServices` is an
        // in-process struct (reset builder + live-debug cell in main.rs resolve
        // it), NOT a network listener — it is not the attack surface the toggle
        // removes. Only the server registration below is gated.
        holon_mcp::di::register_debug_services(injector);

        self.configure_mcp(injector)?;

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        // Same flag as `configure_mcp`: when disabled the handle was never
        // registered, so resolving it would panic — skip the start step.
        let mcp_enabled = self.holon_config.mcp_enabled();
        Box::pin(async move {
            // Frontend: resolve FrontendSession (triggers async factory chain)
            let _session = injector
                .resolve_async::<holon_frontend::FrontendSession>()
                .await;

            // GPUI-specific: populate BuilderServicesSlot + start MCP
            let engine = injector.resolve::<ReactiveEngine>();
            let slot = injector.resolve::<BuilderServicesSlot>();
            let services: Arc<dyn BuilderServices> = engine.clone();
            slot.0.set(services.clone()).ok();

            if mcp_enabled {
                let mcp = injector.resolve::<McpServerHandle>();
                mcp.set_builder_services(services);
                mcp.start().await.map_err(|e| to_di_err("on_start", &e))?;
            }

            Ok(())
        })
    }

    fn on_stop(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        // Symmetric guard: a disabled server was never registered, so there is
        // nothing to resolve or stop.
        let mcp_enabled = self.holon_config.mcp_enabled();
        Box::pin(async move {
            if mcp_enabled {
                let mcp = injector.resolve::<McpServerHandle>();
                mcp.stop().await.map_err(|e| to_di_err("on_stop", &e))?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod mcp_toggle_tests {
    //! The `mcp.enabled` prod toggle (default true) gates the embedded MCP
    //! server at the composition seam (`GpuiModule::configure_mcp`). When
    //! `false`, NO `McpServerHandle` is registered — so `on_start` spawns no
    //! task and nothing binds the loopback listener (attack-surface reduction).
    //!
    //! These assert the registration side directly on a bare injector — no DB,
    //! no window, no runtime — because the security property is decided there:
    //! an unregistered handle cannot be resolved, so the (also-gated)
    //! `on_start` start step is unreachable.
    use std::collections::HashSet;
    use std::path::PathBuf;

    use fluxdi::Injector;
    use holon_frontend::config::HolonConfig;
    use holon_frontend::config::SessionConfig;
    use holon_mcp::di::McpServerHandle;

    use super::GpuiModule;

    fn module_with_mcp_enabled(enabled: Option<bool>) -> GpuiModule {
        let mut holon_config = HolonConfig::default();
        holon_config.mcp.enabled = enabled;
        GpuiModule {
            holon_config,
            session_config: SessionConfig::new(holon_api::UiInfo::permissive()),
            config_dir: PathBuf::from("/tmp/holon-mcp-toggle-test"),
            locked_keys: HashSet::new(),
        }
    }

    #[test]
    fn mcp_disabled_registers_no_server_handle() {
        let injector = Injector::root();
        module_with_mcp_enabled(Some(false))
            .configure_mcp(&injector)
            .expect("configure_mcp must succeed");
        assert!(
            injector.try_resolve::<McpServerHandle>().is_err(),
            "mcp.enabled = false must register NO MCP server handle — nothing to start, nothing \
             to bind"
        );
    }

    #[test]
    fn mcp_enabled_by_default_registers_server_handle() {
        let injector = Injector::root();
        module_with_mcp_enabled(None)
            .configure_mcp(&injector)
            .expect("configure_mcp must succeed");
        assert!(
            injector.try_resolve::<McpServerHandle>().is_ok(),
            "default config (mcp enabled) must register the MCP server handle — behavior unchanged"
        );
    }

    #[test]
    fn mcp_explicitly_enabled_registers_server_handle() {
        let injector = Injector::root();
        module_with_mcp_enabled(Some(true))
            .configure_mcp(&injector)
            .expect("configure_mcp must succeed");
        assert!(
            injector.try_resolve::<McpServerHandle>().is_ok(),
            "mcp.enabled = true must register the MCP server handle"
        );
    }
}
