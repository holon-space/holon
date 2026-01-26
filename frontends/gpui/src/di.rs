//! FluxDI module for the GPUI frontend.
//!
//! `GpuiModule` composes [`CoreInfraModule`] and [`HolonFrontendModule`] via
//! explicit delegation (not `imports()`, which creates child injector scopes
//! that can't see sibling registrations), then adds GPUI-specific services.
//!
//! Lifecycle:
//! - `configure()`: core infra → frontend services → render interpreter → MCP server
//! - `on_start()`: schema init → session resolution → slot population → MCP start
//! - `on_stop()`: graceful MCP server shutdown

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};

use holon::di::CoreInfraModule;
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::HolonFrontendModule;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::{
    BuilderServices, BuilderServicesSlot, ReactiveEngine, RenderInterpreterInjectorExt,
};
use holon_mcp::di::McpServerHandle;
use holon_mcp::McpInjectorExt;

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
}

impl Module for GpuiModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        self.core_module().configure(injector)?;
        self.frontend_module().configure(injector)?;

        // GPUI-specific: render interpreter + debug services
        let slot = injector.resolve::<BuilderServicesSlot>();
        injector.set_render_interpreter(crate::make_interpret_fn(slot.0.clone()));
        holon_mcp::di::register_debug_services(injector);

        let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
            .ok() // ALLOW(ok): non-critical env var
            .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
            .unwrap_or(8520);
        injector.add_mcp_server(mcp_port)?;

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
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
