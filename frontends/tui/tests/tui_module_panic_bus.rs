//! The bus `TuiModule::new` installs the panic hook on is the bus its
//! container resolves.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Injector;
use fluxdi::Module;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_app::vault_lock::SessionVault;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::McpConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_tui::di::TuiModule;

const MESSAGE: &str = "tui module bus probe";

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_reaches_the_bus_the_tui_container_resolves() {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault_root = dir.path().join("vault");
    std::fs::create_dir(&vault_root).expect("create the vault dir");
    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("holon.db")),
        vault: VaultConfig {
            root: Some(vault_root.clone()),
        },
        mcp: McpConfig {
            enabled: Some(false),
        },
        ..Default::default()
    };
    let vault = SessionVault::acquire(Some(&vault_root)).expect("acquire the vault");
    let module = TuiModule::new(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().join("config"),
        HashSet::new(),
        vault,
    );
    let injector = Injector::root();
    module.configure(&injector).expect("TuiModule::configure");
    let bus = injector
        .try_resolve_async::<Arc<ConditionBus>>()
        .await
        .expect("the TUI container's bus");

    let joined = std::thread::spawn(|| panic!("{MESSAGE}")).join();
    assert!(joined.is_err(), "the probe thread must die of its panic");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !bus.current().iter().any(
        |c| matches!(&c.reason, ConditionKind::TaskPanicked { message, .. } if message == MESSAGE),
    ) {
        assert!(
            Instant::now() < deadline,
            "the panic never reached the TUI container's bus; it has {:?}",
            bus.current()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
