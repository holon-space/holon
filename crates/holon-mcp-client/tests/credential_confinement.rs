//! RUNG 1 — a profile's credential reads stay inside that profile.
//!
//! `HOLON_CONFIG_DIR` is the only isolation a sandbox instance has. It is worth
//! nothing if a sidecar's credential declaration resolves against `$HOME`
//! instead: an instance pointed at a throwaway config dir then reads the real
//! user's OAuth refresh token and syncs their real account
//! (`docs/Testing/bugfunnel/entries/
//! 2026-09-01-integration-credentials-escape-config-dir.md`).
//!
//! The property under test is one sentence: **resolving any credential
//! declaration through the production path either yields a location inside the
//! active config dir, or fails.** There is no third outcome — in particular
//! there is no outcome in which a file elsewhere on the machine is opened.
//!
//! Nothing here touches the network, and nothing reads a real credential: the
//! decoy secrets are written into a tempdir by the test itself.

use std::path::Path;
use std::path::PathBuf;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::RestAuth;
use holon_mcp_client::RestOAuth2Config;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::oauth_bootstrap::recorded_credentials;
use proptest::prelude::*;

/// The oauth2 arm a bundled sidecar declares, or `None` for a provider that
/// needs no OAuth at all.
fn bundled_oauth2(provider: &str) -> Option<RestOAuth2Config> {
    let bundled = holon_mcp_client::bundled_sidecar(provider).expect("bundled");
    let config: IntegrationFileConfig =
        serde_yaml::from_str(bundled.yaml).expect("the bundled sidecar parses");
    config.transport.rest?.auth?.oauth2
}

/// Every declared credential location in one resolved credential set.
fn declared_locations(creds: &holon_mcp_client::Credentials) -> Vec<PathBuf> {
    let mut out = vec![creds.refresh_token_file.clone()];
    for r in [&creds.client_id, &creds.client_secret] {
        if let CredentialRef::File { path } = r {
            out.push(path.clone());
        }
    }
    out
}

/// A sidecar oauth2 arm whose three credential files all sit at `declared`,
/// so one generated declaration exercises every arm at once.
fn oauth2_declaring(declared: &str) -> RestOAuth2Config {
    serde_yaml::from_str(&format!(
        "token_url: https://example.invalid/token\n\
         client_id_file: \"{declared}\"\n\
         client_secret_file: \"{declared}\"\n\
         refresh_token_file: \"{declared}\"\n"
    ))
    .expect("the synthesized oauth2 arm parses")
}

/// THE RED. Every bundled OAuth sidecar must name its credentials inside the
/// profile that is running, whatever `$HOME` happens to be.
#[test]
fn a_bundled_sidecar_resolves_its_credentials_inside_the_active_config_dir() {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = CredentialRoot::new(sandbox.path());

    for provider in ["gcal", "gmail"] {
        let cfg = bundled_oauth2(provider)
            .unwrap_or_else(|| panic!("'{provider}' is expected to declare an oauth2 arm"));
        let creds = recorded_credentials(&cfg, &root)
            .unwrap_or_else(|e| panic!("'{provider}' credential locations do not resolve: {e:#}"));

        for path in declared_locations(&creds) {
            assert!(
                path.starts_with(root.path()),
                "'{provider}' resolves a credential to '{}', which is OUTSIDE the active config \
                 dir '{}'. A sandbox instance would authenticate with the real user's account.",
                path.display(),
                root.path().display()
            );
        }
    }
}

/// THE OTHER HALF: a credential that exists outside the config dir must not be
/// read even when it is perfectly valid. The observable is the built
/// transport — an `OAuth2` auth arm can only exist if the decoy secret was
/// opened.
#[test]
fn a_valid_credential_outside_the_config_dir_is_never_read() {
    let sandbox = tempfile::tempdir().expect("sandbox");
    let decoy = tempfile::tempdir().expect("decoy");
    write_private(&decoy.path().join("client-id"), "decoy-client-id");
    write_private(&decoy.path().join("client-secret"), "decoy-client-secret");
    write_private(&decoy.path().join("refresh-token"), "decoy-refresh-token");

    let bundled = holon_mcp_client::bundled_sidecar("gcal").expect("bundled");
    let mut config: IntegrationFileConfig = serde_yaml::from_str(bundled.yaml).expect("parses");
    let oauth2 = config
        .transport
        .rest
        .as_mut()
        .and_then(|r| r.auth.as_mut())
        .and_then(|a| a.oauth2.as_mut())
        .expect("gcal declares an oauth2 arm");
    oauth2.client_id_file = Some(decoy.path().join("client-id").display().to_string());
    oauth2.client_secret_file = Some(decoy.path().join("client-secret").display().to_string());
    oauth2.refresh_token_file = decoy.path().join("refresh-token").display().to_string();

    let root = CredentialRoot::new(sandbox.path());
    let built = config.into_mcp_config_with("gcal".to_string(), &|_| None, &root);

    match built {
        Err(e) => {
            let msg = format!("{e:#}");
            assert!(
                msg.contains(&sandbox.path().display().to_string()),
                "the refusal must name the active config dir '{}' so the user can see which \
                 profile is missing the credential — got: {msg}",
                sandbox.path().display()
            );
        }
        Ok(mcp) => {
            let McpTransport::Rest { manual, .. } = &mcp.transport else {
                panic!("gcal is a rest transport");
            };
            assert!(
                !matches!(manual.auth, RestAuth::OAuth2(_)),
                "gcal authenticated from '{}', which is outside the active config dir '{}' — the \
                 decoy refresh token was read and this instance can now act as that account.",
                decoy.path().display(),
                sandbox.path().display()
            );
        }
    }
}

/// CASE B: the escape that survives a path-only check. The declaration names a
/// file inside the config dir and the parent directory really is the config
/// dir — but the credential NAME is a symlink to another profile's token, and
/// every read primitive follows it. The 0600 check passes too, because it
/// inspects the target's mode, and the victim's token is properly private.
///
/// A path that only reads as confined is not confined. The write leg already
/// knows this (it creates tokens with `O_EXCL` + 0600, never through a link);
/// the read leg must not be the softer half.
#[cfg(unix)]
#[test]
fn a_symlink_at_the_credential_name_is_never_followed_out_of_the_config_dir() {
    let sandbox = tempfile::tempdir().expect("sandbox");
    let victim = tempfile::tempdir().expect("victim profile");
    write_private(&victim.path().join("gcal-client-id"), "victim-client-id");
    write_private(
        &victim.path().join("gcal-client-secret"),
        "victim-client-secret",
    );
    write_private(
        &victim.path().join("gcal-refresh-token"),
        "victim-refresh-token",
    );

    // The sandbox's own config dir, with the victim's credentials linked in
    // under the very names the bundled sidecar declares.
    for name in ["gcal-client-id", "gcal-client-secret", "gcal-refresh-token"] {
        std::os::unix::fs::symlink(victim.path().join(name), sandbox.path().join(name))
            .expect("link the victim's credential into the sandbox");
    }

    let bundled = holon_mcp_client::bundled_sidecar("gcal").expect("bundled");
    let config: IntegrationFileConfig = serde_yaml::from_str(bundled.yaml).expect("parses");
    let root = CredentialRoot::new(sandbox.path());

    match config.into_mcp_config_with("gcal".to_string(), &|_| None, &root) {
        Err(e) => {
            let msg = format!("{e:#}");
            // Any error at all would satisfy a bare `Err` arm, so this rung
            // would keep passing with the link refusal deleted — it must name
            // the link as the reason.
            assert!(
                msg.contains("is a symbolic link"),
                "the refusal must be the LINK refusal, not an unrelated failure that happens to \
                 stop the boot: {msg}"
            );
            assert!(
                !msg.contains("victim-refresh-token") && !msg.contains("victim-client-secret"),
                "the refusal must not echo the secret it refused to use: {msg}"
            );
        }
        Ok(mcp) => {
            let McpTransport::Rest { manual, .. } = &mcp.transport else {
                panic!("gcal is a rest transport");
            };
            assert!(
                !matches!(manual.auth, RestAuth::OAuth2(_)),
                "gcal authenticated through a symlink at '{}' pointing into '{}' — the other \
                 profile's refresh token was read and this instance can now act as that account.",
                sandbox.path().display(),
                victim.path().display()
            );
        }
    }
}

/// The same escape at the resolution boundary, so it is refused before anything
/// is built rather than only at the read.
#[cfg(unix)]
#[test]
fn confining_a_declaration_onto_a_symlinked_credential_name_is_refused() {
    let sandbox = tempfile::tempdir().expect("sandbox");
    let victim = tempfile::tempdir().expect("victim profile");
    write_private(&victim.path().join("token"), "victim-refresh-token");
    std::os::unix::fs::symlink(
        victim.path().join("token"),
        sandbox.path().join("linked-token"),
    )
    .expect("link");

    let err = CredentialRoot::new(sandbox.path())
        .confine("${CONFIG_DIR}/linked-token")
        .expect_err("a credential name that is a link out of the profile must be refused")
        .to_string();
    assert!(err.contains("link"), "{err}");
}

// The general law, over the shapes a declaration can take: resolution either
// refuses, or lands under the root. A `~/` reference and an absolute path
// elsewhere are the two the bug funnel entry was written about; `..` is the
// way round the first two.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_declaration_ever_resolves_outside_the_config_dir(
        declared in prop_oneof![
            "\\$\\{CONFIG_DIR\\}/[a-z-]{1,12}",
            "[a-z-]{1,12}",
            "~/\\.config/holon/[a-z-]{1,12}",
            "~/[a-z-]{1,12}",
            "/etc/[a-z-]{1,12}",
            "/Users/[a-z]{1,8}/\\.config/holon/[a-z-]{1,12}",
            "\\.\\./[a-z-]{1,12}",
            "\\$\\{CONFIG_DIR\\}/\\.\\./\\.\\./[a-z-]{1,12}",
        ],
        profile in "[a-z]{1,8}",
    ) {
        let base = tempfile::tempdir().expect("tempdir");
        let config_dir = base.path().join(profile);
        let root = CredentialRoot::new(&config_dir);

        if let Ok(creds) = recorded_credentials(&oauth2_declaring(&declared), &root) {
            for path in declared_locations(&creds) {
                prop_assert!(
                    path.starts_with(&config_dir),
                    "declaration '{declared}' resolved to '{}', outside the config dir '{}'",
                    path.display(),
                    config_dir.display(),
                );
            }
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, contents).expect("write decoy credential");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("chmod 600 decoy credential");
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write decoy credential");
}
