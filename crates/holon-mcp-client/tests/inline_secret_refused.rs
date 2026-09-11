//! A credential is REFERENCED by name, never written into a sidecar.
//!
//! `HolonSection::auth` has documented "NEVER inline a secret" since the
//! `utcp:` cutover, and nothing enforced it: `auth.static_token` and
//! `holon.auth.value` were plain `String`s, `${VAR}`-expanded, so a literal
//! passed through untouched and became a live credential sitting in a file
//! that gets copied, synced and pasted into bug reports.
//!
//! The rule is one shape, applied to every sidecar including the bundled ones:
//! an auth value is an optional literal prefix followed by exactly one
//! `${VAR}`. `"Bearer ${GITHUB_TOKEN}"` is a reference; `"Bearer ghp_x"` is a
//! secret. No token format is pattern-matched — the absence of a reference IS
//! the defect.

use holon_mcp_client::integration_config::IntegrationFileConfig;

/// Synthetic throughout. A value that looks like a credential must never be a
/// real one, and these exist to be REFUSED, so nothing here reaches a network.
const SYNTHETIC_LITERAL: &str = "SYNTHETIC-NOT-A-REAL-TOKEN-0000";

fn parse(yaml: &str) -> Result<IntegrationFileConfig, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

fn mcp_sidecar_with_token(token: &str) -> String {
    format!(
        r#"
schema_version: 2
transport:
  http:
    uri: https://api.example/mcp
auth:
  static_token: "{token}"
entities: {{}}
tools: {{}}
"#
    )
}

fn manual_sidecar_with_header_value(value: &str) -> String {
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
        url: https://api.example/things
holon:
  auth:
    header: Authorization
    value: "{value}"
  tools:
    list: {{}}
"#
    )
}

#[test]
fn an_inlined_static_token_is_refused_at_parse() {
    let err = parse(&mcp_sidecar_with_token(SYNTHETIC_LITERAL))
        .expect_err("a literal static_token must not parse");
    let msg = err.to_string();
    assert!(
        !msg.contains(SYNTHETIC_LITERAL),
        "the refusal must not echo the secret; got: {msg}"
    );
    assert!(
        msg.contains("${"),
        "the refusal must say what shape was required; got: {msg}"
    );
}

#[test]
fn an_inlined_header_value_is_refused_at_parse() {
    let err = parse(&manual_sidecar_with_header_value(&format!(
        "Bearer {SYNTHETIC_LITERAL}"
    )))
    .expect_err("a literal header value must not parse");
    let msg = err.to_string();
    assert!(
        !msg.contains(SYNTHETIC_LITERAL),
        "the refusal must not echo the secret; got: {msg}"
    );
}

#[test]
fn a_bare_reference_parses() {
    parse(&mcp_sidecar_with_token("${GITHUB_TOKEN}")).expect("a bare ${VAR} is a reference");
}

#[test]
fn a_reference_behind_a_literal_prefix_parses() {
    parse(&manual_sidecar_with_header_value("Bearer ${GITHUB_TOKEN}"))
        .expect("a prefix plus one ${VAR} is a reference");
}

/// Two references in one value would mean two secrets concatenated, which no
/// scheme this transport speaks needs, and a trailing literal after the
/// reference is how a secret gets smuggled past a prefix-only check.
#[test]
fn more_than_one_reference_is_refused() {
    assert!(parse(&mcp_sidecar_with_token("${A}${B}")).is_err());
    assert!(parse(&mcp_sidecar_with_token("${A}-trailing")).is_err());
}

#[test]
fn an_empty_value_is_refused() {
    assert!(parse(&mcp_sidecar_with_token("")).is_err());
}

#[test]
fn an_unterminated_reference_is_refused() {
    assert!(parse(&mcp_sidecar_with_token("${UNCLOSED")).is_err());
}

/// The rule applies to the bundle too, so nothing shipped can quietly hold a
/// literal. `todoist` is the one bundled sidecar with a static token.
#[test]
fn every_bundled_sidecar_references_its_secrets() {
    for bundled in holon_mcp_client::BUNDLED_SIDECARS {
        parse(bundled.yaml).unwrap_or_else(|e| {
            panic!(
                "bundled sidecar '{}' must satisfy the reference-only auth rule: {e}",
                bundled.provider
            )
        });
    }
}
