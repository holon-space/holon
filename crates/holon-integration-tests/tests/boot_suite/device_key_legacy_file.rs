//! A device key file left in the vault is removed unread at boot and never
//! becomes the device identity.
//!
//! @pbt kind harness
//! @pbt covers device-key-legacy-file — a boot over a vault holding a valid
//! `device.key` (and its `.tmp`) leaves neither file and mints an identity of
//! its own
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never writes a
//! file under the Loro storage directory, so it cannot plant a key file there

use std::sync::Arc;

use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// Any 32 bytes are a valid ed25519 secret; this pattern is greppable in logs.
fn planted_key_bytes() -> [u8; 32] {
    std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11))
}

/// Fixed case rather than a generator draw: the planted file is the whole
/// input, and its key-state axes are covered by the custody properties.
#[test]
fn a_planted_device_key_file_is_removed_and_never_becomes_the_identity() {
    let rt = runtime();
    rt.clone().block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("first boot");
        let storage_dir = env
            .injector()
            .expect("injector")
            .resolve::<holon_loro_wiring::LoroConfig>()
            .storage_dir
            .clone();
        env.stop_app().await.expect("stop the app");

        let planted = planted_key_bytes();
        let key_file = storage_dir.join("device.key");
        let tmp_file = storage_dir.join("device.key.tmp");
        std::fs::create_dir_all(&storage_dir).expect("storage dir");
        std::fs::write(&key_file, planted).expect("plant device.key");
        std::fs::write(&tmp_file, planted).expect("plant device.key.tmp");

        env.start_app(true).await.expect("boot over the planted key file");
        let identity = env
            .injector()
            .expect("injector")
            .resolve::<Arc<iroh::SecretKey>>();
        let planted_public = iroh::SecretKey::from_bytes(&planted).public();

        let key_file_kept = key_file.exists();
        let tmp_file_kept = tmp_file.exists();
        let identity_is_planted_key = identity.public() == planted_public;
        assert_eq!(
            (key_file_kept, tmp_file_kept, identity_is_planted_key),
            (false, false, false),
            "(device.key kept, device.key.tmp kept, identity is the planted key)"
        );
    });
}
