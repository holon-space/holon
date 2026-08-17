//! The tri-state integration model and its reactive store.
//!
//! An integration has three independent axes: it is PRESENT (this build
//! bundles it), ENABLED (the user turned it on) and CONFIGURED (credentials
//! exist). Presence is settled at compile time by
//! [`holon_mcp_client::BUNDLED_SIDECARS`]; the other two are user state that
//! survives restarts, so they live in a per-provider file beside the sidecars
//! and reach consumers only as a signal.

use std::fs;
use std::path::Path;

use futures::StreamExt;
use futures_signals::signal::SignalExt;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::integration_state::Credentials;
use holon_mcp_client::integration_state::IntegrationConfigStore;
use holon_mcp_client::integration_state::IntegrationState;
use proptest::prelude::*;

fn credential_ref_strategy() -> impl Strategy<Value = CredentialRef> {
    prop_oneof![
        "[A-Z_]{1,12}".prop_map(|var| CredentialRef::Env { var }),
        "/[a-z0-9/._-]{1,20}".prop_map(|p| CredentialRef::File { path: p.into() }),
        ("[a-z]{1,10}", "[a-z]{1,10}")
            .prop_map(|(service, account)| CredentialRef::Keychain { service, account }),
    ]
}

fn configuration_strategy() -> impl Strategy<Value = Configuration> {
    prop_oneof![
        Just(Configuration::Unconfigured),
        (
            credential_ref_strategy(),
            credential_ref_strategy(),
            "/[a-z0-9/._-]{1,20}",
        )
            .prop_map(|(client_id, client_secret, refresh_token_file)| {
                Configuration::Configured(Credentials {
                    client_id,
                    client_secret,
                    refresh_token_file: refresh_token_file.into(),
                })
            }),
    ]
}

fn state_strategy() -> impl Strategy<Value = IntegrationState> {
    (any::<bool>(), configuration_strategy()).prop_map(|(enabled, configuration)| {
        IntegrationState {
            enabled,
            configuration,
        }
    })
}

/// Every state a consumer can write survives a full trip through the
/// filesystem: a fresh store built over the same directory reads back exactly
/// what was written, for every bundled provider.
#[test]
fn every_state_round_trips_through_a_fresh_store() {
    proptest!(|(states in proptest::collection::vec(state_strategy(), 5))| {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = IntegrationConfigStore::load(dir.path()).expect("load empty");
        let providers = store.providers();
        prop_assert!(!providers.is_empty(), "this build must bundle sidecars");

        for (provider, state) in providers.iter().zip(states.iter()) {
            store.set(provider, state.clone()).expect("set");
        }

        let reloaded = IntegrationConfigStore::load(dir.path()).expect("reload");
        for (provider, state) in providers.iter().zip(states.iter()) {
            prop_assert_eq!(&reloaded.get(provider).expect("get"), state);
        }
    });
}

/// A write reaches in-process consumers through the signal, not only the file.
#[tokio::test]
async fn a_write_pushes_the_new_state_into_the_signal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");

    let mut stream = store
        .state("gcal")
        .expect("gcal is bundled")
        .signal_cloned()
        .to_stream();

    let first = stream.next().await.expect("current value");
    assert_eq!(first, IntegrationState::default());

    store
        .set(
            "gcal",
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("set");

    let second = stream.next().await.expect("updated value");
    assert!(second.enabled, "the signal must carry the write");
}

/// An integration nobody ever touched is off and unconfigured — the absence of
/// a state file is a state, not a parse failure.
#[test]
fn a_never_touched_integration_is_disabled_and_unconfigured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");

    for provider in store.providers() {
        let state = store.get(provider).expect("get");
        assert!(!state.enabled, "{provider} must default off");
        assert_eq!(state.configuration, Configuration::Unconfigured);
    }
}

/// The canonical on-disk form for an enabled, configured integration, produced
/// by the store itself so the probe table below cannot drift from what `set`
/// actually writes.
fn canonical_state_file(dir: &Path) -> String {
    let store = IntegrationConfigStore::load(dir).expect("load");
    store
        .set(
            "todoist",
            IntegrationState {
                enabled: true,
                configuration: Configuration::Configured(Credentials {
                    client_id: CredentialRef::Env {
                        var: "CID".to_string(),
                    },
                    client_secret: CredentialRef::Keychain {
                        service: "svc".to_string(),
                        account: "acct".to_string(),
                    },
                    refresh_token_file: "/tmp/rt".into(),
                }),
            },
        )
        .expect("set");
    fs::read_to_string(dir.join("todoist.state.toml")).expect("read back")
}

/// A state file that EXISTS but is not a complete, current-schema state is a
/// hard error naming the provider and the path.
///
/// Only a MISSING file means "never touched". Anything else that loaded as the
/// default would quietly switch a configured integration off — and the widest
/// such window is the zero-byte file an interrupted write leaves behind, so
/// every degradation below has to be fatal, not just the type mismatch serde
/// rejects on its own.
#[test]
fn every_corrupt_state_file_fails_loud_with_provider_and_path() {
    let canonical_dir = tempfile::tempdir().expect("tempdir");
    let canonical = canonical_state_file(canonical_dir.path());

    let probes: Vec<(&str, String)> = vec![
        ("empty file (interrupted write)", String::new()),
        ("newline only", "\n".to_string()),
        (
            "truncated after the first line",
            canonical.lines().next().expect("first line").to_string() + "\n",
        ),
        ("typo'd key", "enabledd = true\n".to_string()),
        ("renamed key", "enable = true\n".to_string()),
        ("unknown extra key", format!("whatever = 1\n{canonical}")),
        ("type mismatch", "enabled = \"yes, very\"\n".to_string()),
        (
            "future schema version",
            canonical.replace("schema_version = 1", "schema_version = 2"),
        ),
        (
            "no schema version",
            canonical
                .lines()
                .filter(|l| !l.starts_with("schema_version"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ];

    let mut escaped = Vec::new();
    for (label, content) in probes {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("todoist.state.toml");
        fs::write(&path, &content).expect("write");

        match IntegrationConfigStore::load(dir.path()).map(|s| s.get("todoist").expect("get")) {
            Ok(state) => escaped.push(format!("{label}: loaded silently as {state:?}")),
            Err(e) => {
                let msg = format!("{e:#}");
                if !msg.contains("todoist") || !msg.contains(&path.display().to_string()) {
                    escaped.push(format!(
                        "{label}: failed loud but the message names neither the provider nor the \
                         path: {msg}"
                    ));
                }
            }
        }
    }
    assert!(
        escaped.is_empty(),
        "{} of the probe inputs degraded silently:\n  {}",
        escaped.len(),
        escaped.join("\n  ")
    );
}

/// `set` must not be able to leave a half-written state file behind: it writes
/// a temporary sibling and renames it over the target, so an interruption
/// leaves either the previous complete state or nothing at all.
#[test]
fn a_write_leaves_no_partial_file_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");
    store
        .set(
            "gmail",
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("set");

    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n != "gmail.state.toml")
        .collect();
    assert!(
        leftovers.is_empty(),
        "set must leave no temporary residue: {leftovers:?}"
    );
    assert!(
        !fs::read_to_string(dir.path().join("gmail.state.toml"))
            .expect("read")
            .is_empty(),
        "the written state file must never be observable as empty"
    );
}

/// Presence is compile-time: a provider this build does not bundle has no
/// state to read or write, and asking for one is an error rather than an
/// invented default.
#[test]
fn a_provider_this_build_does_not_bundle_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");

    let err = store.get("linear").expect_err("unbundled provider");
    assert!(format!("{err:#}").contains("linear"));
    assert!(
        store.set("linear", IntegrationState::default()).is_err(),
        "writing state for an unbundled provider must fail"
    );
}

/// State files live in the same directory as the sidecars, so they must stay
/// invisible to the sidecar scan — a `gcal.state.*` file must never be read as
/// a provider named `gcal.state`.
#[test]
fn state_files_are_invisible_to_the_sidecar_scan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");
    for provider in store.providers() {
        store
            .set(
                provider,
                IntegrationState {
                    enabled: true,
                    configuration: Configuration::Unconfigured,
                },
            )
            .expect("set");
    }

    let loaded = holon_mcp_client::load_integration_configs(dir.path(), &store)
        .expect("scan must not choke");
    let mut loaded_names: Vec<&str> = loaded.configs.iter().map(|(n, _)| n.as_str()).collect();
    loaded_names.sort_unstable();
    let mut expected = store.providers();
    expected.sort_unstable();
    assert_eq!(
        loaded_names, expected,
        "a `gcal.state.toml` enables `gcal`, and is never itself read as a \
         provider named `gcal.state`"
    );
    assert!(loaded.superseded.is_empty());
    assert!(loaded.ignored.is_empty());
}

/// The store writes state beside the sidecars, under the name the loader skips.
#[test]
fn state_lands_beside_the_sidecar_it_describes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("load");
    store
        .set(
            "gmail",
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("set");

    let expected: &Path = &dir.path().join("gmail.state.toml");
    assert!(expected.exists(), "state file must exist at {expected:?}");
}
