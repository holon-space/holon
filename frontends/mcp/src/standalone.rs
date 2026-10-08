//! The DI module the standalone MCP binary boots.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::ModuleLifecycleFuture;
use fluxdi::Shared;
use holon_app::FrontendInjectorExt;

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("McpStandaloneModule", phase, &e.to_string())
}

pub struct McpStandaloneModule {
    holon_config: holon_frontend::HolonConfig,
    session_config: holon_frontend::SessionConfig,
    config_dir: PathBuf,
    conditions: Arc<holon_api::ConditionBus>,
}

impl McpStandaloneModule {
    /// Its bus is the one panics are raised on.
    pub fn new(
        holon_config: holon_frontend::HolonConfig,
        session_config: holon_frontend::SessionConfig,
        config_dir: PathBuf,
    ) -> Self {
        Self {
            conditions: holon_frontend::panic_record::install(&config_dir),
            holon_config,
            session_config,
            config_dir,
        }
    }
}

impl Module for McpStandaloneModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        let vault =
            holon_app::vault_lock::SessionVault::acquire(self.holon_config.vault.root.as_deref())
                .map_err(|e| to_di_err("configure", &format!("{e:#}")))?;
        let db_path = self.holon_config.resolve_db_path(&self.config_dir);

        holon::di::open_and_register_core(
            injector,
            db_path,
            holon::di::StorageSelector::Turso,
            self.conditions.clone(),
        )
        .map_err(|e| to_di_err("configure", &e))?;
        vault.register(injector);

        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                HashSet::new(),
            )
            .map_err(|e| to_di_err("configure", &e))?;

        holon_mcp::di::register_debug_services(injector);

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            holon_mcp::di::populate_debug_services(&injector, None).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use holon_api::ConditionBus;
    use holon_api::ConditionKind;
    use holon_frontend::config::VaultConfig;

    use super::*;

    const MESSAGE: &str = "mcp standalone bus probe";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_panic_reaches_the_bus_the_session_resolves() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).expect("create the vault dir");
        let holon_config = holon_frontend::HolonConfig {
            db_path: Some(dir.path().join("holon.db")),
            vault: VaultConfig { root: Some(vault) },
            ..Default::default()
        };
        let module = McpStandaloneModule::new(
            holon_config,
            holon_frontend::SessionConfig::new(holon_api::UiInfo::permissive()),
            dir.path().join("config"),
        );
        let injector = Injector::root();
        module
            .configure(&injector)
            .expect("McpStandaloneModule::configure");
        let bus = injector
            .try_resolve_async::<Arc<ConditionBus>>()
            .await
            .expect("the session's bus");

        let joined = std::thread::spawn(|| panic!("{MESSAGE}")).join();
        assert!(joined.is_err(), "the probe thread must die of its panic");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !bus.current().iter().any(|c| {
            matches!(&c.reason, ConditionKind::TaskPanicked { message, .. } if message == MESSAGE)
        }) {
            assert!(
                Instant::now() < deadline,
                "the panic never reached the bus the session resolves; it has {:?}",
                bus.current()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
