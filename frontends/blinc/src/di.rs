//! FluxDI module for the Blinc frontend.
//!
//! - `configure()`: core infrastructure, frontend services, no-op render interpreter, MCP server
//! - `on_start()`: schema init, async resolution, MCP server start
//! - `on_stop()`: graceful MCP server shutdown

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};

use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::FrontendInjectorExt;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::FrontendSession;
use holon_mcp::McpInjectorExt;
use holon_mcp::di::McpServerHandle;

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("BlincModule", phase, &e.to_string())
}

pub struct BlincModule {
    pub holon_config: HolonConfig,
    pub session_config: SessionConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<PrefKey>,
}

impl Module for BlincModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        let db_path = self.holon_config.resolve_db_path(&self.config_dir);

        holon::di::open_and_register_core(injector, db_path)
            .map_err(|e| to_di_err("configure", &e))?;

        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                self.locked_keys.clone(),
            )
            .map_err(|e| to_di_err("configure", &e))?;

        injector.set_render_interpreter(|_expr, _rows| {
            holon_frontend::reactive_view_model::ReactiveViewModel::empty()
        });

        injector.add_mcp_server(8520)?;

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            let mcp = injector.resolve::<McpServerHandle>();
            mcp.start()
                .await
                .map_err(|e| to_di_err("on_start", &e))?;

            Ok(())
        })
    }

    fn on_stop(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let mcp = injector.resolve::<McpServerHandle>();
            mcp.stop()
                .await
                .map_err(|e| to_di_err("on_stop", &e))?;
            Ok(())
        })
    }
}
