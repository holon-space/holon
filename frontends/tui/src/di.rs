//! FluxDI module for the TUI frontend.
//!
//! Mirrors the GPUI module structure: `configure()` registers core infra,
//! frontend services, the render interpreter, and the MCP server;
//! `on_start()` populates the BuilderServicesSlot with the resolved
//! ReactiveEngine and starts the MCP server; `on_stop()` shuts the MCP
//! server down gracefully.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::ModuleLifecycleFuture;
use fluxdi::Shared;
use holon_app::FrontendInjectorExt;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::BuilderServicesSlot;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::reactive::make_interpret_fn;
use holon_mcp::McpInjectorExt;
use holon_mcp::di::McpServerHandle;

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("TuiModule", phase, &e.to_string())
}

pub struct TuiModule {
    pub holon_config: HolonConfig,
    pub session_config: SessionConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<PrefKey>,
}

impl Module for TuiModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        let db_path = self.holon_config.resolve_db_path(&self.config_dir);

        holon::di::open_and_register_core(injector, db_path, holon::di::StorageSelector::Turso)
            .map_err(|e| to_di_err("configure", &e))?;

        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                self.locked_keys.clone(),
            )
            .map_err(|e| to_di_err("configure", &e))?;

        let slot = injector.resolve::<BuilderServicesSlot>();
        injector.set_render_interpreter(make_interpret_fn(slot.0.clone()));

        holon_mcp::di::register_debug_services(injector);

        let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
            .ok() // ALLOW(ok): non-critical env var
            .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
            .unwrap_or(8520);
        injector.add_mcp_server(mcp_port)?;

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        let crdt_enabled = self.holon_config.crdt_enabled();
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            let engine = injector.resolve::<ReactiveEngine>();
            let slot = injector.resolve::<BuilderServicesSlot>();
            let services: Arc<dyn BuilderServices> = engine.clone();
            slot.0.set(services.clone()).ok();

            // Wire `BlockCellRegistry` (Loro-backed when LoroModule is
            // loaded) into the ReactiveEngine so `BuilderServices::
            // editable_text` resolves through `Cell<String>`. SqlOnly
            // mode skips this — `editable_text` returns the no-cell branch and
            // editors run without a CRDT-backed cell.
            //
            // Through the SHARED installer, so the TUI cannot boot a subtly
            // different frontend than production or the harnesses.
            holon_app::loro_seams::install_block_cell_registry(&injector, &engine, crdt_enabled)
                .await
                .map_err(|e| to_di_err("on_start", &e))?;

            let mcp = injector.resolve::<McpServerHandle>();
            mcp.set_builder_services(services);
            mcp.start().await.map_err(|e| to_di_err("on_start", &e))?;

            Ok(())
        })
    }

    fn on_stop(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let mcp = injector.resolve::<McpServerHandle>();
            mcp.stop().await.map_err(|e| to_di_err("on_stop", &e))?;
            Ok(())
        })
    }
}
