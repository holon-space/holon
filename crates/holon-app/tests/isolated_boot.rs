//! The shipped boot with the device key held in memory, keyed by the config
//! dir so a restart over one dir finds the key its predecessor minted. Without
//! it the boot loads the key from the platform keychain, which a test process
//! is refused, and sharing stays off.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::preferences::PrefKey;

pub async fn new_from_config_with_di<F, G, T>(
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
    conditions: Arc<holon_conditions::ConditionBus>,
    extra_setup: F,
    extra_resolve: G,
) -> anyhow::Result<(Arc<FrontendSession>, Arc<BackendEngine>, T)>
where
    F: FnOnce(&fluxdi::Injector) -> anyhow::Result<()> + Send + 'static,
    G: FnOnce(&fluxdi::Injector) -> T + Send + 'static,
    T: Send + 'static,
{
    let device_store: Arc<dyn holon_secrets::KeychainStore> =
        holon_loro::device_key_custody::testing::shared_store(&config_dir.to_string_lossy());
    holon_app::new_from_config_with_di(
        holon_config,
        session_config,
        config_dir,
        locked_keys,
        conditions,
        move |injector| {
            injector.override_provider::<holon_secrets::DeviceKeychain>(fluxdi::Provider::root(
                move |_| Arc::new(holon_secrets::DeviceKeychain(device_store.clone())),
            ));
            extra_setup(injector)
        },
        extra_resolve,
    )
    .await
}
