//! Integration enablement is reachable through the ONE action language.
//!
//! Before `IntegrationsOperationProvider`, the only door onto the enablement
//! store was the GPUI switch's own mouse handler calling
//! `IntegrationsSettingsVm::set_enabled` — an ADR-0024 violation that also made
//! the surface unreachable from MCP, from a test driver, and from an agent.
//!
//! These tests drive the REAL dispatch path: `McpIntegrationsModule` registers
//! the provider exactly as it does in the app, and every write goes through
//! `BackendEngine::execute_operation`. A test that constructed the provider by
//! hand would keep passing after the DI registration was dropped, which is the
//! escape this file exists to prevent.
//!
//! @pbt kind harness
//! @pbt covers integration-set-field-operation — dispatching
//! `integration.set_field` flips the state file (the AUTHORITY), and every
//! malformed dispatch is refused with a message naming the offending value
//! @pbt overlaps integration_state_projection — kept: that file pins the
//! store → table MIRROR; this one pins the operation → store WRITE

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_mcp_client::IntegrationConfigStore;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// A fresh engine whose dispatcher carries whatever `McpIntegrationsModule`
/// registers over `state_dir` — the prod wiring, not a hand-built provider.
///
/// The container's OWN store comes back with it. Two stores over one directory
/// share the files but not the signals (`IntegrationsSettingsVm::over_dir`), so
/// a test that seeded through a second store would be arranging a state the
/// composition root cannot produce.
async fn engine_over(
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
) -> (Arc<holon::api::BackendEngine>, Arc<IntegrationConfigStore>) {
    let (engine, store) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            holon_app::mcp_integrations::McpIntegrationsModule::from_dir(&state_dir)
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure McpIntegrationsModule for op test: {e}"))
        },
        |injector| async move { injector.resolve_async::<IntegrationConfigStore>().await },
    )
    .await
    .expect("fresh-db lazy DI graph must build");
    (engine, store)
}

/// `field`/`value` as the toggle sends them: a state WORD, never a bool.
fn params(id: &str, field: &str, value: &str) -> StorageEntity {
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("field".into(), Value::String(field.to_string()));
    p.insert("value".into(), Value::String(value.to_string()));
    p
}

async fn dispatch(
    engine: &holon::api::BackendEngine,
    id: &str,
    field: &str,
    value: &str,
) -> anyhow::Result<()> {
    engine
        .execute_operation(
            &EntityName::new("integration"),
            "set_field",
            params(id, field, value),
            OpOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// What the AUTHORITY — the state file — says, read through a fresh store so
/// the assertion cannot be satisfied by an in-memory cell alone.
fn stored_enabled(state_dir: &std::path::Path, provider: &str) -> bool {
    IntegrationConfigStore::load(state_dir)
        .expect("store reloads from disk")
        .get(provider)
        .unwrap_or_else(|e| panic!("provider '{provider}' has no state: {e:#}"))
        .enabled
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_dir = dir.path().join("integrations");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    (dir, state_dir)
}

/// THE RED. Without the provider registered, the dispatcher answers "no
/// provider for entity 'integration'" and the state file never moves.
#[test]
fn dispatching_set_field_flips_the_state_file() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let (engine, _store) = engine_over(dir.path().join("fresh.db"), state_dir.clone()).await;

        assert!(
            !stored_enabled(&state_dir, "todoist"),
            "fixture must start with todoist switched off"
        );

        dispatch(&engine, "integration:todoist", "enabled", "on")
            .await
            .expect("dispatching integration.set_field must succeed");
        assert!(
            stored_enabled(&state_dir, "todoist"),
            "the operation must write the AUTHORITY — the .state.toml file"
        );

        dispatch(&engine, "integration:todoist", "enabled", "off")
            .await
            .expect("dispatching the off direction must succeed");
        assert!(
            !stored_enabled(&state_dir, "todoist"),
            "the operation must switch back off, not only on"
        );
    });
}

/// Enablement and configuration are orthogonal axes with separate doors. A
/// `set_field` that carried the configuration axis could discard a consent some
/// providers will not grant twice, so the store must keep it across a switch.
#[test]
fn switching_leaves_the_configuration_axis_untouched() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let (engine, store) = engine_over(dir.path().join("fresh.db"), state_dir.clone()).await;

        let configured = holon_mcp_client::integration_state::IntegrationState {
            enabled: false,
            configuration: holon_mcp_client::integration_state::Configuration::Configured(
                holon_mcp_client::integration_state::Credentials {
                    client_id: holon_mcp_client::integration_state::CredentialRef::Env {
                        var: "HOLON_TEST_CLIENT_ID".to_string(),
                    },
                    client_secret: holon_mcp_client::integration_state::CredentialRef::Env {
                        var: "HOLON_TEST_CLIENT_SECRET".to_string(),
                    },
                    refresh_token_file: state_dir.join("gmail.refresh"),
                },
            ),
        };
        store
            .set("gmail", configured.clone())
            .expect("seed a configured-but-off gmail");

        dispatch(&engine, "integration:gmail", "enabled", "on")
            .await
            .expect("switching a configured integration on must succeed");

        let after = IntegrationConfigStore::load(&state_dir)
            .expect("store reloads")
            .get("gmail")
            .expect("gmail state");
        assert!(after.enabled, "the enablement axis must have moved");
        assert_eq!(
            after.configuration, configured.configuration,
            "the configuration axis must survive a switch verbatim"
        );
    });
}

/// The refusal matrix. Every malformed dispatch must `Err` with a message
/// naming the offending value — never a silent no-op, and never a coercion.
#[test]
fn every_malformed_dispatch_is_refused_by_name() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let (engine, _store) = engine_over(dir.path().join("fresh.db"), state_dir.clone()).await;

        // (id, field, value, the substring the refusal must carry)
        let cases = [
            ("block:abc123", "enabled", "on", "block"),
            (
                "integration:not-a-provider",
                "enabled",
                "on",
                "not-a-provider",
            ),
            ("todoist", "enabled", "on", "todoist"),
            (
                "integration:todoist",
                "configuration",
                "on",
                "configuration",
            ),
            ("integration:todoist", "enabled", "yes", "yes"),
        ];

        for (id, field, value, expected) in cases {
            let err = dispatch(&engine, id, field, value)
                .await
                .expect_err(&format!(
                    "dispatch({id:?}, {field:?}, {value:?}) must be refused, not accepted"
                ));
            let text = format!("{err:#}");
            assert!(
                text.contains(expected),
                "the refusal for ({id:?}, {field:?}, {value:?}) must name {expected:?}; got: {text}"
            );
        }

        assert!(
            !stored_enabled(&state_dir, "todoist"),
            "no refused dispatch may have written the state file"
        );
    });
}

/// An unwritable state directory is a refusal the user can act on, not a
/// switch that appears to move and silently does not.
#[test]
fn an_unwritable_state_directory_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let (engine, _store) = engine_over(dir.path().join("fresh.db"), state_dir.clone()).await;

        let mut perms = std::fs::metadata(&state_dir)
            .expect("state dir metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o555);
        }
        std::fs::set_permissions(&state_dir, perms).expect("make the state dir read-only");

        let err = dispatch(&engine, "integration:todoist", "enabled", "on")
            .await
            .expect_err("a write into a read-only directory must be refused");
        let text = format!("{err:#}");
        assert!(
            text.contains("todoist"),
            "the refusal must name the provider whose decision could not be stored; got: {text}"
        );

        // Restore so the tempdir can be cleaned up.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore permissions");
        }
    });
}
