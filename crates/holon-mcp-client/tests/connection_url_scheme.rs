//! A connection's calls go over TLS, or to this machine, and nowhere else.
//!
//! ADR 0034 §6 lists "a non-HTTPS non-localhost URL" among the things a
//! sidecar load refuses. Nothing enforced it: the guard existed only for the
//! OAuth consent endpoints (`oauth_bootstrap::parse_secure_endpoint`), while a
//! `utcp:` manual's own `tool_call_template.url` was expanded and used as
//! written.
//!
//! That is load-bearing now. A manual's calls carry the connection's auth
//! header, so a cleartext URL puts the token on the wire in the clear for
//! anyone on the path, and an on-path attacker can rewrite the response that
//! becomes rows in the user's vault.
//!
//! Loopback is the exception, for the reason RFC 8252 §7.3 gives the consent
//! flow and because every mock server in this suite is a loopback server: the
//! request never leaves the machine.

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::integration_config::IntegrationFileConfig;

fn credential_root() -> CredentialRoot {
    CredentialRoot::new("/tmp/holon-connection-url-scheme-test")
}

/// A one-tool manual whose single call goes to `url`.
fn sidecar_calling(url: &str) -> String {
    format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: list
      tool_call_template:
        call_template_type: http
        http_method: GET
        url: "{url}"
holon:
  tools:
    list: {{}}
"#
    )
}

fn build(url: &str) -> anyhow::Result<()> {
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&sidecar_calling(url)).expect("the fixture sidecar parses");
    cfg.into_mcp_config_with("fixture".to_string(), &|_| None, &credential_root())
        .map(|_| ())
}

#[test]
fn a_cleartext_url_is_refused() {
    let err = build("http://evil.example/api")
        .expect_err("a cleartext call URL must not build a transport");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https"),
        "the refusal must say what was required; got: {msg}"
    );
    assert!(
        msg.contains("list"),
        "the refusal must name the tool whose URL is at fault; got: {msg}"
    );
}

#[test]
fn an_https_url_is_accepted() {
    build("https://api.example/things").expect("an https call URL must build");
}

/// Both loopback spellings, because a mock server binds one and a hand-written
/// sidecar is likelier to write the other.
#[test]
fn a_loopback_url_is_accepted() {
    build("http://127.0.0.1:8080/api").expect("loopback by address must build");
    build("http://localhost:8080/api").expect("loopback by name must build");
}

/// `file://` reads the user's disk and `ftp://` is cleartext; neither is a
/// scheme this transport can drive, and refusing by an allowlist rather than a
/// blocklist is what makes that true for the next scheme too.
#[test]
fn a_non_http_scheme_is_refused() {
    for url in ["file:///etc/passwd", "ftp://example.com/x", "gopher://x/1"] {
        assert!(build(url).is_err(), "{url} must not build a call transport");
    }
}

/// A URL that is not a URL must fail at the boundary rather than at request
/// time, where the failure would look like a dead connection.
#[test]
fn a_malformed_url_is_refused() {
    assert!(build("not a url").is_err());
}

/// The refusal must not quote the expanded URL: an expanded URL can BE the
/// secret (the shopping sidecar's whole endpoint is `${SHOPPING_LIST_URL}`, a
/// capability URL), so a message echoing it would put the credential in the
/// log the refusal writes.
#[test]
fn the_refusal_does_not_echo_an_expanded_secret_url() {
    let secret = "http://host.example/!SYNTHETIC-CAPABILITY-TOKEN/api";
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&sidecar_calling("${LIST_URL}")).expect("the fixture sidecar parses");
    let err = cfg
        .into_mcp_config_with(
            "fixture".to_string(),
            &|name| (name == "LIST_URL").then(|| secret.to_string()),
            &credential_root(),
        )
        .expect_err("a cleartext call URL must not build a transport");
    let msg = format!("{err:#}");
    assert!(
        !msg.contains("SYNTHETIC-CAPABILITY-TOKEN"),
        "the refusal must not echo the expanded URL; got: {msg}"
    );
}

/// Every sidecar this build ships already satisfies the rule, so turning it on
/// costs nothing — and this pins that they keep satisfying it.
#[test]
fn every_bundled_sidecar_satisfies_the_rule() {
    for bundled in holon_mcp_client::BUNDLED_SIDECARS {
        let cfg: IntegrationFileConfig = serde_yaml::from_str(bundled.yaml)
            .unwrap_or_else(|e| panic!("bundled '{}' parses: {e:#}", bundled.provider));
        // Resolve every `${VAR}` to an https placeholder: this test is about
        // the scheme rule, and an unresolved var is a different refusal.
        let built = cfg.into_mcp_config_with(
            bundled.provider.to_string(),
            &|_| Some("https://var.example/endpoint".to_string()),
            &credential_root(),
        );
        if let Err(e) = built {
            let msg = format!("{e:#}");
            assert!(
                !msg.contains("must use https"),
                "bundled sidecar '{}' violates the call-URL scheme rule: {msg}",
                bundled.provider
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The MCP transport URI is the same wire, and needs the same guard.
// ---------------------------------------------------------------------------

/// `transport.http.uri` carries the connection's auth token exactly as a
/// manual call does (`mcp_integration.rs` sends `AuthMode::StaticToken` on it),
/// so a cleartext URI there puts the credential on the wire in the clear. The
/// manual's `url` was guarded first; this field is the other half of the same
/// hole, and it is reachable by an installed override for a BUNDLED stem.
fn mcp_sidecar_calling(uri: &str) -> String {
    format!(
        r#"
schema_version: 2
transport:
  http:
    uri: "{uri}"
auth:
  static_token: "${{SOME_TOKEN}}"
entities: {{}}
tools: {{}}
"#
    )
}

fn build_mcp(uri: &str) -> anyhow::Result<()> {
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&mcp_sidecar_calling(uri)).expect("the fixture sidecar parses");
    cfg.into_mcp_config_with(
        "fixture".to_string(),
        &|_| Some("SYNTHETIC-TOKEN-VALUE".to_string()),
        &credential_root(),
    )
    .map(|_| ())
}

#[test]
fn a_cleartext_mcp_transport_uri_is_refused() {
    let err = build_mcp("http://evil.example/mcp")
        .expect_err("a cleartext MCP transport URI must not build");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https"),
        "the refusal must say what was required; got: {msg}"
    );
    assert!(
        msg.contains("transport.http.uri"),
        "the refusal must name the field at fault; got: {msg}"
    );
    assert!(
        !msg.contains("evil.example"),
        "the refusal must not echo the URI, which may carry a credential; got: {msg}"
    );
}

#[test]
fn an_https_mcp_transport_uri_is_accepted() {
    build_mcp("https://api.example/mcp").expect("an https MCP transport URI must build");
}

/// A local MCP server over loopback is how this is developed and tested.
#[test]
fn a_loopback_mcp_transport_uri_is_accepted() {
    build_mcp("http://127.0.0.1:9000/mcp").expect("loopback must build");
    build_mcp("http://localhost:9000/mcp").expect("loopback by name must build");
}

#[test]
fn a_non_http_mcp_transport_uri_is_refused() {
    for uri in ["file:///etc/passwd", "ftp://example.com/x", "not a url"] {
        assert!(build_mcp(uri).is_err(), "{uri} must not build a transport");
    }
}
