//! The shopping list URL reaches the connector from the Settings menu, and is
//! treated as the credential it is on the way.
//!
//! The URL contains the list's capability token in a path segment, so this
//! rung spans the whole seam the user's keystroke travels: the preference
//! `holon.toml` persists → the layered `${VAR}` resolver → the REST transport
//! manual the connector runs on → the redactor that guards every string that
//! transport emits. Only `holon-app` sees both ends.
//!
//! Every URL here is synthetic. A real list URL is a live credential and never
//! belongs in a fixture, even a private one.

use std::collections::HashMap;
use std::path::PathBuf;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::load_config;
use holon_frontend::integration_vars::normalize_var_name;
use holon_frontend::integration_vars::preference_var_lookup;
use holon_frontend::preferences::PrefKey;
use holon_frontend::preferences::PrefType;
use holon_frontend::preferences::define_preferences;
use holon_frontend::preferences::env_shadowed_keys;
use holon_frontend::theme::ThemeRegistry;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::Redactor;

/// The preference the Settings menu writes.
const PREF_KEY: &str = "shopping.list_url";

/// The variable `assets/integrations/shopping.yaml` references.
const ENV_VAR: &str = "SHOPPING_LIST_URL";

/// The capability token of the URL a user pastes into Settings. Nothing else in
/// this file contains this literal, so any occurrence in a transport string is
/// a leak.
///
/// It sits in an UNMARKED path segment on purpose. The redactor also blanks
/// `!`-marked segments structurally, without having been told the value, so a
/// marked token would go whether or not the Settings value was ever registered
/// — and this rung is about the registration.
const PREF_TOKEN: &str = "abc123SYNTHETICprefTOKENq7Wv";

/// The token of a URL exported in the environment, to tell the two layers apart
/// when both are configured.
const ENV_TOKEN: &str = "abc123SYNTHETICenvTOKENz4Kd";

fn pref_url() -> String {
    format!("https://shop.example/c/{PREF_TOKEN}/api")
}

fn env_url() -> String {
    format!("https://shop.example/c/{ENV_TOKEN}/api")
}

/// The bundled sidecar the app actually runs, not a restatement of it: the
/// agreement this rung is about is between the Settings key and the `${VAR}`
/// THAT file names.
fn shopping_sidecar() -> IntegrationFileConfig {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/integrations/shopping.yaml");
    let yaml =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn preferences(url: &str) -> HashMap<PrefKey, toml::Value> {
    HashMap::from([(PrefKey::new(PREF_KEY), toml::Value::String(url.into()))])
}

/// Resolve the sidecar into a transport manual through the production path.
fn manual_from(
    prefs: &HashMap<PrefKey, toml::Value>,
    env: impl Fn(&str) -> Option<String> + Send + Sync,
) -> holon_mcp_client::rest_transport::RestManual {
    let lookup = preference_var_lookup(prefs, env);
    let built = shopping_sidecar()
        .into_mcp_config_with(
            "shopping".to_string(),
            &lookup,
            &CredentialRoot::new("/tmp/holon-c2-settings-no-credential-files"),
        )
        .expect("the sidecar resolves once the list URL is configured");
    match built.transport {
        McpTransport::Rest { manual, .. } => manual,
        other => panic!("shopping.yaml declares a utcp connection, got {other:?}"),
    }
}

/// The list URL as the manual resolved it. The share link is the `pull_list`
/// tool's own `url` — a UTCP manual states an absolute URL per tool and has no
/// base to share.
fn resolved_url(manual: &holon_mcp_client::rest_transport::RestManual) -> &str {
    &manual
        .calls
        .get("pull_list")
        .expect("the shopping manual declares `pull_list`")
        .url
}

#[test]
fn the_settings_preference_configures_the_connector_with_no_environment_variable() {
    let manual = manual_from(&preferences(&pref_url()), |_| None);
    assert_eq!(
        resolved_url(&manual),
        pref_url(),
        "a URL set in Settings must configure the connector on its own — needing an export is \
         exactly what this preference removes"
    );
}

#[test]
fn an_exported_variable_outranks_the_settings_value() {
    let manual = manual_from(&preferences(&pref_url()), |name| {
        (name == ENV_VAR).then(|| env_url())
    });
    assert_eq!(
        resolved_url(&manual),
        env_url(),
        "the environment layer is the outer one; a sandbox launched with an exported credential \
         must not silently run on the persisted profile's"
    );
}

#[test]
fn the_shadowed_settings_field_is_read_only_rather_than_silently_ignored() {
    let defs = define_preferences(&ThemeRegistry::load(None));

    let unshadowed = env_shadowed_keys(&defs, &|_| None);
    assert!(
        !unshadowed.contains(&PrefKey::new(PREF_KEY)),
        "with nothing exported the field is the user's to set"
    );

    let shadowed = env_shadowed_keys(&defs, &|name| (name == ENV_VAR).then(|| env_url()));
    assert!(
        shadowed.contains(&PrefKey::new(PREF_KEY)),
        "an export outranks the field, so Settings must show it read-only — an editable field \
         holding a credential nothing reads leaves the user unable to tell which of two secrets \
         is in force"
    );
}

#[test]
fn a_blank_export_does_not_take_the_field_away() {
    let defs = define_preferences(&ThemeRegistry::load(None));
    let shadowed = env_shadowed_keys(&defs, &|name| (name == ENV_VAR).then(String::new));
    assert!(
        !shadowed.contains(&PrefKey::new(PREF_KEY)),
        "an exported-but-empty variable configures nothing, so it must not lock the field"
    );
}

#[test]
fn the_settings_key_and_the_sidecars_variable_are_the_same_name() {
    let defs = define_preferences(&ThemeRegistry::load(None));
    let def = defs
        .iter()
        .find(|d| d.key.as_str() == PREF_KEY)
        .expect("the shopping list URL is a settings-menu field");

    assert!(
        matches!(def.pref_type, PrefType::Secret),
        "the URL carries the list's capability token, so the field must be masked"
    );
    assert_eq!(
        def.env_override,
        Some(ENV_VAR),
        "the declared override must be the variable the sidecar references"
    );
    assert_eq!(
        normalize_var_name(def.key.as_str()),
        normalize_var_name(ENV_VAR),
        "the resolver matches the key against the variable name; a key that normalizes to \
         anything else would leave the Settings field feeding nothing"
    );
}

#[test]
fn the_pasted_token_is_registered_as_a_secret_and_never_reaches_an_error_string() {
    let manual = manual_from(&preferences(&pref_url()), |_| None);

    // The shapes an upstream failure actually carries the URL in: the request
    // line, and a body that echoes back the path it was asked for.
    let request_line = format!("GET {}/list/7 failed: 502", resolved_url(&manual));
    let echoed_body = format!(r#"{{"error":"no route for /c/{PREF_TOKEN}/api/list/7"}}"#);
    let bare_token = format!("upstream rejected {PREF_TOKEN}");

    for message in [&request_line, &echoed_body, &bare_token] {
        // The negative control: without the registration this rung is about,
        // the token stands. It is what stops the assertion below from passing
        // for a reason other than the one it names.
        assert!(
            Redactor::new().redact(message).contains(PREF_TOKEN),
            "an unregistered token must survive, or the assertion below proves nothing: {message}"
        );

        let redacted = manual.redactor.redact(message);
        assert!(
            !redacted.contains(PREF_TOKEN),
            "a list URL supplied through Settings must be registered with the redactor exactly as \
             an exported one is. leaked in: {redacted}"
        );
    }

    assert!(
        !manual
            .redactor
            .redact_url(&request_line)
            .contains(PREF_TOKEN),
        "the URL-shaped redaction path must strip it too"
    );
}

/// The leg every other rung here stubs: the preference arrives from a REAL
/// `holon.toml` through the desktop boot loader, not from a map built in
/// memory. A user pastes the URL into Settings, restarts (the field is
/// `requires_restart`), and the connector must come up configured.
#[test]
fn a_list_url_persisted_in_holon_toml_configures_the_connector_after_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir for a throwaway config profile");
    // Exactly the shape `HolonConfig::save_runtime` writes.
    std::fs::write(
        dir.path().join("holon.toml"),
        format!("[preferences]\n\"{PREF_KEY}\" = \"{}\"\n", pref_url()),
    )
    .expect("write holon.toml");

    let (traced, _locked) = load_config(dir.path(), HolonConfig::default())
        .expect("the desktop boot loader must accept this config");
    let booted = traced.into_inner();

    assert_eq!(
        booted
            .preferences
            .get(&PrefKey::new(PREF_KEY))
            .and_then(|v| v.as_str()),
        Some(pref_url().as_str()),
        "the stored preference must survive the boot load, or every credential typed into \
         Settings is forgotten on restart. whole map: {:?}",
        booted.preferences
    );

    let manual = manual_from(&booted.preferences, |_| None);
    assert_eq!(
        resolved_url(&manual),
        pref_url(),
        "the connector must be configured from the persisted preference alone, with no \
         environment variable set"
    );
    assert!(
        !manual
            .redactor
            .redact(&format!("GET {}/list/7 failed", resolved_url(&manual)))
            .contains(PREF_TOKEN),
        "a URL that arrived from disk must be registered with the redactor exactly as an \
         exported one is"
    );
}
