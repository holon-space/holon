//! FluxDI module for the TUI frontend.
//!
//! - `configure()`: core infrastructure, frontend services, no-op render interpreter
//! - `on_start()`: schema init, async resolution

use std::collections::HashSet;
use std::path::PathBuf;

use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};

use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::FrontendInjectorExt;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::FrontendSession;

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

        injector.set_render_interpreter(|_expr, _rows| holon_frontend::ReactiveViewModel::empty());

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            Ok(())
        })
    }
}
