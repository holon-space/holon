//! The calendar URL reaches the ICS connector from the Settings menu, and is
//! treated as the credential it is on the way.
//!
//! A published "secret address in iCal format" is a bearer credential in a URL:
//! anyone holding the address reads the whole calendar. So this rung spans the
//! seam a keystroke travels: the preference `holon.toml` persists → the layered
//! `${VAR}` resolver → the REST transport manual the connector runs on → the
//! redactor that guards every string that transport emits. Only `holon-app`
//! sees both ends.
//!
//! It also pins the agreement between the SHIPPED sidecar and the engine: the
//! bundled `ics-calendar.yaml` must declare the `ics` codec and a bounded
//! expansion window, because a sidecar that quietly fell back to the `json`
//! codec would produce an empty replica rather than an error.
//!
//! Every URL here is synthetic. A real calendar address is a live credential
//! and never belongs in a fixture, even a private one.

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
use holon_mcp_client::rest_transport::ResponseFormat;

/// The preference the Settings menu writes.
const PREF_KEY: &str = "ics.calendar_url";

/// The variable `assets/integrations/ics-calendar.yaml` references.
const ENV_VAR: &str = "ICS_CALENDAR_URL";

/// The provider name the bundled sidecar's stem gives it.
const PROVIDER: &str = "ics-calendar";

/// The manual's one tool.
const TOOL: &str = "list-events";

/// The capability token of the address a user pastes into Settings. Nothing
/// else in this file contains this literal, so any occurrence in a transport
/// string is a leak.
const PREF_TOKEN: &str = "icsSYNTHETICprefTOKENm3Qz";

/// The token of an address exported in the environment, to tell the two layers
/// apart when both are configured.
const ENV_TOKEN: &str = "icsSYNTHETICenvTOKENv8Rd";

fn pref_url() -> String {
    format!("https://calendar.example/ical/{PREF_TOKEN}/basic.ics")
}

fn env_url() -> String {
    format!("https://calendar.example/ical/{ENV_TOKEN}/basic.ics")
}

/// The bundled sidecar the app actually runs, not a restatement of it: the
/// agreement this rung is about is between the Settings key and the `${VAR}`
/// THAT file names.
fn ics_sidecar() -> IntegrationFileConfig {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/integrations/ics-calendar.yaml");
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
    // No stored secret: this test is about the preference layer, and an empty
    // keychain is what a fresh profile has.
    let keychain = holon_secrets::InMemoryKeychainStore::new();
    let lookup = preference_var_lookup(prefs, env, &keychain);
    let built = ics_sidecar()
        .into_mcp_config_with(
            PROVIDER.to_string(),
            &lookup,
            &CredentialRoot::new("/tmp/holon-ics-settings-no-credential-files"),
        )
        .expect("the sidecar resolves once the calendar URL is configured");
    match built.transport {
        McpTransport::Rest { manual, .. } => manual,
        other => panic!("ics-calendar.yaml declares a utcp connection, got {other:?}"),
    }
}

/// The calendar URL as the manual resolved it. A UTCP manual states an absolute
/// URL per tool and has no base to share, so the feed address IS the tool
/// `url`.
fn resolved_url(manual: &holon_mcp_client::rest_transport::RestManual) -> &str {
    &manual
        .calls
        .get(TOOL)
        .expect("the ICS manual declares `list-events`")
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
        (name == ENV_VAR).then(env_url)
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

    let shadowed = env_shadowed_keys(&defs, &|name| (name == ENV_VAR).then(env_url));
    assert!(
        shadowed.contains(&PrefKey::new(PREF_KEY)),
        "an export outranks the field, so Settings must show it read-only — an editable field \
         holding a credential nothing reads leaves the user unable to tell which of two secrets \
         is in force"
    );
}

#[test]
fn the_settings_key_and_the_sidecars_variable_are_the_same_name() {
    let defs = define_preferences(&ThemeRegistry::load(None));
    let def = defs
        .iter()
        .find(|d| d.key.as_str() == PREF_KEY)
        .expect("the calendar URL is a settings-menu field");

    assert!(
        matches!(def.pref_type, PrefType::Secret),
        "the address is a bearer credential for the whole calendar, so the field must be masked"
    );
    assert_eq!(
        def.env_override.as_deref(),
        Some(ENV_VAR),
        "the declared override must be the variable the sidecar references"
    );
    assert_eq!(
        normalize_var_name(def.key.as_str()),
        normalize_var_name(ENV_VAR),
        "the resolver matches the key against the variable name; a key that normalizes to \
         anything else would leave the Settings field feeding nothing"
    );
    assert_eq!(
        def.section.as_str(),
        "Integrations",
        "a connection credential belongs in the Integrations section beside the other ones"
    );
}

/// The shipped sidecar and the engine must agree on the codec and on the bound.
/// A sidecar that lost `format: ics` would decode a calendar document as JSON
/// and fail with a parse error at best; one that lost its window would expand a
/// decade of recurrences.
#[test]
fn the_shipped_sidecar_declares_the_ics_codec_and_a_bounded_window() {
    let manual = manual_from(&preferences(&pref_url()), |_| None);
    let call = manual.calls.get(TOOL).expect("`list-events` is declared");

    assert_eq!(
        call.format,
        ResponseFormat::Ics,
        "the bundled sidecar must name the `ics` codec; the default is `json`, and a calendar \
         document is not JSON"
    );
    assert!(
        call.ics_window.back.duration().as_secs() > 0,
        "expansion must be bounded behind too, or the bound is not a bound"
    );
    assert!(
        call.ics_window.forward.duration().as_secs() > 0,
        "expansion must be bounded ahead, or a feed that is the whole history expands without \
         limit"
    );
    assert_eq!(
        call.result_key.as_deref(),
        Some("entries"),
        "the decoded occurrence array is wrapped under this key for `sync.extract_path`"
    );

    let cfg = ics_sidecar();
    assert_eq!(
        cfg.entity_prefix.as_deref(),
        Some("ics_"),
        "the mirror tables are namespaced by the sidecar's prefix; an empty one would collide \
         with whatever else declares an `event` table"
    );
}

#[test]
fn the_pasted_url_is_registered_as_a_secret_and_never_reaches_an_error_string() {
    let manual = manual_from(&preferences(&pref_url()), |_| None);

    // The shapes an upstream failure actually carries the URL in: the request
    // line, and a body that echoes back the path it was asked for.
    let request_line = format!("GET {} failed: 404", resolved_url(&manual));
    let echoed_body = format!(r#"{{"error":"no route for /ical/{PREF_TOKEN}/basic.ics"}}"#);
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
            "a calendar URL supplied through Settings must be registered with the redactor \
             exactly as an exported one is. leaked in: {redacted}"
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
/// memory. A user pastes the address into Settings, restarts (the field is
/// `requires_restart`), and the connector must come up configured.
#[test]
fn an_ics_url_persisted_in_holon_toml_configures_the_connector_after_a_restart() {
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
            .redact(&format!("GET {} failed", resolved_url(&manual)))
            .contains(PREF_TOKEN),
        "a URL that arrived from disk must be registered with the redactor exactly as an \
         exported one is"
    );
}
