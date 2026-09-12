//! A session can be told to hold its secrets in RAM, and must say so.
//!
//! Why this exists: the `user-connections` dogfood pass could not drive the
//! keychain half of the credential path at all. The only injectable secret
//! store was reachable in-process (`FrontendSession::use_secret_store`), so a
//! pass driving the BUILT binary had two options — skip those flows, or write
//! the developer's real login keychain. It skipped them, and the "stored in the
//! keychain" wording shipped unverified.
//!
//! What keeps this out of a real install is asserted here as hard as the
//! feature itself: the mode is admitted only over a throwaway config directory
//! (or with an explicit acknowledgement), and when it IS admitted the boot
//! raises a sticky banner. A credential field that silently saves nothing is
//! the degradation this project forbids outright.
//!
//! `holon-secrets` owns the admission RULE and tests it directly; this file
//! pins that a real boot applies it and that the banner reaches the bus.
//!
//! @pbt kind harness
//! @pbt covers in-memory-secret-backend-disclosed — an env-selected in-memory
//! secret backend boots, works, and raises a sticky disclosure

use std::collections::HashSet;
use std::sync::Arc;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_loro::DegradedSignalBus;
use holon_loro::ShareDegradedReason;

/// A value that could only come from this test, so a leak into the real
/// keychain would be findable. Nothing here ever reaches a platform backend:
/// that is what the assertions are for.
const FIXTURE_SECRET: &str = "SYNTHETIC-fixture-token-not-a-credential";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

async fn boot(
    dir: &std::path::Path,
) -> (Arc<holon_frontend::FrontendSession>, Arc<DegradedSignalBus>) {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("secrets.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, bus) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |injector| {
            injector
                .try_resolve::<Arc<DegradedSignalBus>>()
                .map(|b| (*b).clone())
                .expect("the composition root registers a DegradedSignalBus")
        },
    )
    .await
    .expect("the session must boot on the in-memory secret backend");
    (session, bus)
}

#[test]
fn a_throwaway_session_holds_secrets_in_memory_and_discloses_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    // SAFETY: nextest runs each test in its own process, so this variable is
    // this test's alone and is set before the session boots.
    unsafe { std::env::set_var(holon_secrets::BACKEND_ENV, "memory") };

    let (session, bus) = runtime().block_on(boot(dir.path()));

    let key = holon_frontend::preferences::PrefKey::new("todoist.api_key");
    session
        .set_preference(&key, toml::Value::String(FIXTURE_SECRET.to_string()))
        .expect("a secret preference must be storable on the in-memory backend");
    assert!(
        session.secret_is_stored(&holon_secrets::secret_account(key.as_str())),
        "and must read back as stored, or the mode exercises nothing"
    );

    let current = bus.subscribe().current;
    let disclosure = current
        .iter()
        .find_map(|c| match &c.reason {
            ShareDegradedReason::SecretsHeldInMemory { why } => Some(why.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "a session that saves no credential must raise a sticky banner saying so — \
                 without it the user types a token into a field that quietly discards it. The \
                 bus carries {:?}",
                current
                    .iter()
                    .map(|c| c.reason.condition_kind())
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        disclosure.contains("gone when Holon exits"),
        "the banner must say the secrets do not survive the session, which is the fact that \
         changes what the user does; got {disclosure:?}"
    );

    assert!(
        !disclosure.contains(FIXTURE_SECRET),
        "and it must not quote the secret it is talking about"
    );
    drop(session);
}

/// The refusal, through a real boot. A config directory that is not obviously
/// throwaway must stop the boot rather than quietly fall back to the login
/// keychain — a fixture that believed it was isolated and was writing real
/// credentials is the outcome the whole seam exists to prevent.
#[test]
fn a_real_config_dir_refuses_to_boot_in_memory() {
    let dir = tempfile::tempdir().expect("tempdir");
    // SAFETY: as above — one process per test, set before the boot.
    unsafe { std::env::set_var(holon_secrets::BACKEND_ENV, "memory") };

    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("secrets.db")),
        ..Default::default()
    };
    let booted = runtime().block_on(async {
        holon_app::new_from_config_with_di(
            holon_config,
            SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
            std::path::PathBuf::from("/Users/someone/.config/holon"),
            HashSet::new(),
            |_| Ok(()),
            |_| (),
        )
        .await
        .map(|_| ())
    });

    let err = booted.expect_err("a non-throwaway config dir must refuse the in-memory backend");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(holon_secrets::BACKEND_UNSAFE_ENV),
        "and the refusal must name the one way to mean it anyway; got {msg}"
    );
}
