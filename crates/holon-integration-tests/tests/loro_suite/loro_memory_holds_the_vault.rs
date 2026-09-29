//! The Turso-free test wiring writes its temp vault like every other
//! session, so it holds the vault's writer lock too, and its stop releases it
//! through the production `shutdown_session`.
//!
//! @pbt kind harness
//! @pbt covers vault-single-writer — the LoroMemory test session holds its
//! vault and stops through `shutdown_session`

use std::sync::Arc;

use holon::di::StorageSelector;
use holon_app::vault_lock::VaultLock;
use holon_integration_tests::TestEnvironment;

#[test]
fn a_loro_memory_session_holds_its_vault_until_it_stops() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(async move {
        let mut env = TestEnvironment::new_with_backend(runtime, StorageSelector::LoroMemory)
            .expect("new_with_backend(LoroMemory)");
        env.start_app(false).await.expect("start_app (LoroMemory)");
        let vault = env.org_root().to_path_buf();

        assert!(
            VaultLock::acquire(&vault).is_err(),
            "a LoroMemory session runs on {} without holding its writer lock",
            vault.display()
        );

        env.stop_app().await.expect("stop_app (LoroMemory)");
        VaultLock::acquire(&vault).expect("a stopped LoroMemory session releases its vault");
    });
}
