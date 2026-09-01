//! RUNG 2 — `unconfigured` is a gate, not a label.
//!
//! ENABLEMENT and CONFIGURATION are two axes
//! ([`holon_mcp_client::integration_state`]). Switching an OAuth integration ON
//! says the user wants it; it does not say the one-time consent flow has ever
//! run in THIS profile. Until it has, the integration must do NOTHING: no
//! transport, no credential read, no sync, and no claim of being connected.
//!
//! Before this rung, `unconfigured` was decoration — the loader consulted only
//! `enabled`, the connector resolved whatever ambient credentials it could
//! find, and the sidebar reported `Connected` for an account the user never
//! configured here.
//!
//! An integration that needs no OAuth (a public API, a static token from the
//! environment or from settings) is untouched by this gate: its credentials do
//! not come through the consent flow, so `unconfigured` says nothing about it.

use std::path::Path;
use std::path::PathBuf;

use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::LoadedIntegrations;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::integration_state::Credentials;
use holon_mcp_client::integration_state::IntegrationState;
use proptest::prelude::*;

/// The bundled providers whose sidecar declares an OAuth2 arm — the population
/// the gate governs.
const OAUTH_PROVIDERS: &[&str] = &["gcal", "gmail"];

/// The bundled providers that authenticate some other way (or not at all).
const NON_OAUTH_PROVIDERS: &[&str] = &["claude-history", "jsonplaceholder", "todoist"];

fn load(dir: &Path) -> anyhow::Result<LoadedIntegrations> {
    let store = IntegrationConfigStore::load(dir)?;
    holon_mcp_client::load_integration_configs(
        dir,
        &store,
        &holon_mcp_client::CredentialRoot::new(dir),
    )
}

fn set_state(dir: &Path, provider: &str, state: IntegrationState) {
    IntegrationConfigStore::load(dir)
        .expect("store loads")
        .set(provider, state)
        .expect("state is written");
}

/// A credential set of the shape a completed consent flow records.
fn configured_credentials(dir: &Path) -> Credentials {
    Credentials {
        client_id: CredentialRef::File {
            path: dir.join("client-id"),
        },
        client_secret: CredentialRef::File {
            path: dir.join("client-secret"),
        },
        refresh_token_file: dir.join("refresh-token"),
    }
}

fn ran(loaded: &LoadedIntegrations, provider: &str) -> bool {
    loaded.configs.iter().any(|(n, _)| n == provider)
}

/// THE RED. Enabled but never configured in this profile: the loader must
/// produce no config for it, so nothing is ever built that could sync.
#[test]
fn an_enabled_but_unconfigured_oauth_provider_does_not_run() {
    for provider in OAUTH_PROVIDERS {
        let dir = tempfile::tempdir().expect("tempdir");
        set_state(
            dir.path(),
            provider,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        );

        let loaded = load(dir.path()).expect("load");
        assert!(
            !ran(&loaded, provider),
            "'{provider}' is enabled but UNCONFIGURED in this profile, yet the loader produced a \
             config for it — a transport will be built and a real account synced.",
        );
    }
}

/// The gate must also SAY so. An integration that silently does nothing is
/// indistinguishable from one quietly reaching a real account.
#[test]
fn an_inert_provider_is_disclosed_with_its_remedy() {
    let dir = tempfile::tempdir().expect("tempdir");
    set_state(
        dir.path(),
        "gcal",
        IntegrationState {
            enabled: true,
            configuration: Configuration::Unconfigured,
        },
    );

    let loaded = load(dir.path()).expect("load");
    let inert = loaded
        .inert
        .iter()
        .find(|i| i.provider == "gcal")
        .unwrap_or_else(|| {
            panic!(
                "'gcal' was held back but not disclosed — inert: {:?}",
                loaded.inert
            )
        });
    assert!(
        !inert.remedy.trim().is_empty(),
        "the disclosure must name the affordance that configures it",
    );
    assert_eq!(
        inert.state_path,
        PathBuf::from(dir.path()).join("gcal.state.toml"),
        "the disclosure must name the state file whose configuration axis is unset",
    );
}

/// The gate is about CREDENTIALS, not about enablement: once the consent flow
/// has recorded credentials for this profile, the provider runs as before.
#[test]
fn a_configured_oauth_provider_runs() {
    for provider in OAUTH_PROVIDERS {
        let dir = tempfile::tempdir().expect("tempdir");
        set_state(
            dir.path(),
            provider,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Configured(configured_credentials(dir.path())),
            },
        );

        let loaded = load(dir.path()).expect("load");
        assert!(
            ran(&loaded, provider),
            "'{provider}' is enabled AND configured, so it must run",
        );
        assert!(
            loaded.inert.is_empty(),
            "nothing is held back: {:?}",
            loaded.inert
        );
    }
}

// The truth table, over both populations at once: an integration runs iff it
// is enabled AND (it needs no consent flow OR this profile has completed one).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn running_requires_enablement_and_credentials_only_where_credentials_are_needed(
        provider_index in 0usize..(OAUTH_PROVIDERS.len() + NON_OAUTH_PROVIDERS.len()),
        enabled in any::<bool>(),
        configured in any::<bool>(),
    ) {
        let needs_credentials = provider_index < OAUTH_PROVIDERS.len();
        let provider = if needs_credentials {
            OAUTH_PROVIDERS[provider_index]
        } else {
            NON_OAUTH_PROVIDERS[provider_index - OAUTH_PROVIDERS.len()]
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let configuration = if configured {
            Configuration::Configured(configured_credentials(dir.path()))
        } else {
            Configuration::Unconfigured
        };
        set_state(dir.path(), provider, IntegrationState { enabled, configuration });

        let loaded = load(dir.path()).expect("load");
        let expected = enabled && (!needs_credentials || configured);
        prop_assert_eq!(
            ran(&loaded, provider),
            expected,
            "provider '{}' (needs_credentials={}, enabled={}, configured={}) should{} run",
            provider,
            needs_credentials,
            enabled,
            configured,
            if expected { "" } else { " NOT" },
        );
    }
}
