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
    let (engine, store, _vm) =
        engine_over_with_browser(db_path, state_dir, Arc::new(NoBrowser)).await;
    (engine, store)
}

struct NoBrowser;

impl holon_mcp_client::oauth_bootstrap::BrowserOpener for NoBrowser {
    fn open(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn engine_over_with_browser(
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
    browser: Arc<dyn holon_mcp_client::oauth_bootstrap::BrowserOpener>,
) -> (
    Arc<holon::api::BackendEngine>,
    Arc<IntegrationConfigStore>,
    Arc<holon_app::integrations_settings::IntegrationsSettingsVm>,
) {
    let (engine, (store, vm)) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            holon_app::mcp_integrations::McpIntegrationsModule::from_dir(&state_dir)
                .with_browser(browser.clone())
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure McpIntegrationsModule for op test: {e}"))
        },
        |injector| async move {
            let store = injector.resolve_async::<IntegrationConfigStore>().await;
            let vm = injector
                .resolve_async::<holon_app::integrations_settings::IntegrationsSettingsVm>()
                .await;
            (store, vm)
        },
    )
    .await
    .expect("fresh-db lazy DI graph must build");
    (engine, store, vm)
}

/// `field`/`value` as the toggle sends them. `value` is typed by the caller so
/// the refusal cases can send something that is not a bool.
fn params(id: &str, field: &str, value: Value) -> StorageEntity {
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("field".into(), Value::String(field.to_string()));
    p.insert("value".into(), value);
    p
}

async fn dispatch(
    engine: &holon::api::BackendEngine,
    id: &str,
    field: &str,
    value: Value,
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

        dispatch(
            &engine,
            "integration:todoist",
            "enabled",
            Value::Boolean(true),
        )
        .await
        .expect("dispatching integration.set_field must succeed");
        assert!(
            stored_enabled(&state_dir, "todoist"),
            "the operation must write the AUTHORITY — the .state.toml file"
        );

        dispatch(
            &engine,
            "integration:todoist",
            "enabled",
            Value::Boolean(false),
        )
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

        dispatch(
            &engine,
            "integration:gmail",
            "enabled",
            Value::Boolean(true),
        )
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
            ("block:abc123", "enabled", Value::Boolean(true), "block"),
            (
                "integration:not-a-provider",
                "enabled",
                Value::Boolean(true),
                "not-a-provider",
            ),
            ("todoist", "enabled", Value::Boolean(true), "todoist"),
            (
                "integration:todoist",
                "configuration",
                Value::Boolean(true),
                "configuration",
            ),
            (
                "integration:todoist",
                "enabled",
                Value::String("yes".into()),
                "yes",
            ),
            (
                "integration:todoist",
                "enabled",
                Value::String("on".into()),
                "on",
            ),
        ];

        for (id, field, value, expected) in cases {
            let err = dispatch(&engine, id, field, value.clone())
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

        let err = dispatch(
            &engine,
            "integration:todoist",
            "enabled",
            Value::Boolean(true),
        )
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

/// Install a `gcal` sidecar whose credential paths sit inside `dir`, plus the
/// client credentials it points at, so a consent flow gets past credential
/// resolution and parks at the loopback instead of failing early.
fn install_provisioned_gcal(dir: &std::path::Path) {
    let bundled =
        holon_mcp_client::bundled_sidecars::bundled_sidecar("gcal").expect("gcal is bundled");
    let sandboxed = bundled.yaml.replace(
        "~/.config/holon/",
        &format!("{}/", dir.join("creds").display()),
    );
    std::fs::write(dir.join("gcal.yaml"), sandboxed).expect("install sandboxed sidecar");

    let creds = dir.join("creds");
    std::fs::create_dir_all(&creds).expect("creds dir");
    for (name, value) in [
        ("gcal-client-id", "sandbox-client-id.apps.example.com"),
        ("gcal-client-secret", "sandbox-client-secret"),
    ] {
        let path = creds.join(name);
        std::fs::write(&path, value).expect("write credential");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("lock the credential down");
        }
    }
}

/// The consent flow waits on a human for up to five minutes. Dispatching it
/// must START it and return, so the dispatcher is free and the row can say what
/// the flow is doing.
#[test]
fn dispatching_begin_oauth_returns_before_the_flow_finishes() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        install_provisioned_gcal(&state_dir);

        // The view model comes from the CONTAINER: the flow's progress lives in
        // its cells, so a second one over the same files would observe nothing.
        let (engine, _store, vm) = engine_over_with_browser(
            dir.path().join("ops.db"),
            state_dir.clone(),
            Arc::new(NoBrowser),
        )
        .await;

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("integration:gcal".into()));

        let started = std::time::Instant::now();
        engine
            .execute_operation(
                &EntityName::new("integration"),
                "begin_oauth",
                params,
                OpOrigin::User,
            )
            .await
            .expect("dispatching integration.begin_oauth must succeed");
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "begin_oauth must return once the flow is STARTED, not once it finishes; it took \
             {elapsed:?}"
        );

        // The flow it started is the one the mirror observes: the DI-registered
        // view model, not a second one built over the same files.
        let progress = vm.configure_progress("gcal");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if progress.get_cloned()
                == holon_app::integrations_settings::ConfigureProgress::AwaitingConsent
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!(
            "the started flow never reached AwaitingConsent; progress is {:?}",
            progress.get_cloned()
        );
    });
}
