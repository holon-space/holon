//! Sharing is unreachable with the CRDT layer off, and the refusal must say so.
//!
//! `tree.share_subtree` is served by `LoroShareBackend`, which the composition
//! root registers only inside the `crdt_enabled()` branch of `add_frontend`.
//! With the layer off the dispatcher finds no provider and answers "No provider
//! registered for entity: tree" — accurate about the symptom, silent about the
//! cause, and indistinguishable from a broken build (bugfunnel
//! 2026-09-02-desktop-sharing-unreachable-by-default…). The refusal must name
//! the setting that turned it off.
//!
//! The companion half is the SHIPPED default: a config with no
//! `[crdt]` section boots the layer on, so sharing is reachable out of the box.
//!
//! @pbt kind harness
//! @pbt covers sharing-unavailable-names-its-cause — a share dispatched with
//! `crdt.enabled = false` fails loud naming the setting, not the registration
//! @pbt covers desktop-default-boots-crdt — the shipped default config wires
//! the `tree` provider, so a share reaches the backend
//! @pbt overlaps platform_capabilities_disclosure — kept: that file pins the
//! boot-step ledger; this one pins the dispatch refusal

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_frontend::config::CrdtPreferences;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime"),
    )
}

/// Boot the shared wiring over a fresh vault and dispatch a share, returning
/// whatever the dispatcher answered. `crdt` is threaded exactly as a
/// `holon.toml` would supply it: `None` is the shipped default.
async fn share_through_shipped_wiring(
    dir: &std::path::Path,
    crdt: Option<bool>,
) -> anyhow::Result<()> {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("sharing.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        crdt: CrdtPreferences {
            enabled: crdt,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_session, engine, ()) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |_| (),
    )
    .await
    .expect("the shared wiring must boot a session");

    let mut params: StorageEntity = HashMap::new();
    params.insert(
        "id".into(),
        Value::String(format!("block:{}", uuid::Uuid::new_v4())),
    );
    params.insert("retention".into(), Value::String("none".into()));

    engine
        .execute_operation(
            &EntityName::new("tree"),
            "share_subtree",
            params,
            OpOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// THE RED. Before the fix the message was "No provider registered for entity:
/// tree" — it never mentioned the setting that removed the provider.
#[test]
fn sharing_with_crdt_off_names_the_setting() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = share_through_shipped_wiring(dir.path(), Some(false))
            .await
            .expect_err("sharing must fail with the CRDT layer off");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("crdt.enabled"),
            "the refusal must name the setting that disabled sharing, got: {msg}"
        );
    });
}

/// Non-vacuity for the red above AND the D69.a default: with no `[crdt]` key
/// the shipped wiring registers the `tree` provider, so the dispatch reaches
/// the backend. It still fails — the block id is fabricated — but never with
/// the unavailability refusal.
#[test]
fn sharing_reaches_the_backend_on_the_shipped_default() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let outcome = share_through_shipped_wiring(dir.path(), None).await;
        let msg = match &outcome {
            Ok(()) => String::new(),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            !msg.contains("No provider registered"),
            "the shipped default must wire the sharing provider, got: {msg}"
        );
        assert!(
            !msg.contains("crdt.enabled"),
            "the shipped default must not report the CRDT layer as disabled, got: {msg}"
        );
    });
}
