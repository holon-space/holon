//! The `utcp:` section is a manual, not a Holon dialect wearing the name.
//!
//! Two claims, both load-bearing for D84.d: a published UTCP 1.x manual read
//! into Holon's types and written back out is the SAME BYTES (so an import is
//! reversible and a user is never locked in), and the shipped sidecar's `utcp:`
//! section IS that manual (so the standard half of the file is the standard's,
//! not ours).

use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::utcp_manual::UtcpManual;

/// A manual published for the shopping-list peer, as a standard client would
/// hold it.
const MANUAL: &str = include_str!("fixtures/that-shopping-list.utcp.json");

const SHOPPING_SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

#[test]
fn a_manual_written_back_out_is_the_bytes_it_came_in_as() {
    let manual: UtcpManual = serde_json::from_str(MANUAL).expect("the manual parses");
    let exported = serde_json::to_string_pretty(&manual).expect("the manual serializes");
    assert_eq!(
        format!("{exported}\n"),
        MANUAL,
        "exporting the manual changed it; a lossy type here means an imported manual cannot be \
         given back"
    );
}

#[test]
fn the_shopping_sidecars_utcp_section_is_that_manual() {
    let published: UtcpManual = serde_json::from_str(MANUAL).expect("the manual parses");
    let sidecar: IntegrationFileConfig =
        serde_yaml::from_str(SHOPPING_SIDECAR).expect("the shipped sidecar parses");
    let embedded = sidecar
        .utcp
        .expect("the shopping sidecar declares a `utcp:` manual");
    assert_eq!(
        embedded, published,
        "the sidecar's `utcp:` section has drifted from the manual it is supposed to be"
    );
}

#[test]
fn a_call_template_holon_cannot_drive_names_itself_when_it_is_skipped() {
    let mut manual: UtcpManual = serde_json::from_str(MANUAL).expect("the manual parses");
    manual.tools[0].tool_call_template.call_template_type = "cli".to_string();
    let why = manual.tools[0]
        .tool_call_template
        .unsupported_reason(&manual.tools[0].name)
        .expect("a `cli` template is not servable by this build");
    assert!(
        why.contains("cli") && why.contains("pull_list") && why.contains("skipped"),
        "the disclosure must name the transport, the tool, and what happens to it, got: {why}"
    );
    assert!(
        manual.tools[1]
            .tool_call_template
            .unsupported_reason(&manual.tools[1].name)
            .is_none(),
        "an http tool beside it stays servable"
    );
}

#[test]
fn a_holon_entry_for_a_tool_the_manual_does_not_declare_is_refused() {
    let doctored = SHOPPING_SIDECAR.replace("    pull_list:\n", "    pull_lst:\n");
    assert_ne!(doctored, SHOPPING_SIDECAR, "the typo was actually injected");
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&doctored).expect("the doctored sidecar still parses");
    let err = cfg
        .into_mcp_config_with(
            "shopping".to_string(),
            &|name: &str| (name == "SHOPPING_LIST_URL").then(|| "https://example.test/x".into()),
            &holon_mcp_client::credential_path::CredentialRoot::new("/tmp/holon-utcp-roundtrip"),
        )
        .expect_err("a mapping for a tool that does not exist can never run");
    assert!(
        format!("{err:#}").contains("pull_lst"),
        "the refusal must name the offending key, got: {err:#}"
    );
}

/// A manual carrying four keys this build does not model — one at each level
/// the spec has one — plus a tool it cannot drive.
const FORWARD: &str = r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  info:
    title: Things
  tools:
    - name: get-things
      auth:
        auth_type: api_key
      query_params: [limit]
      tool_call_template:
        call_template_type: http
        url: https://example.invalid/things
        http_method: GET
        headers:
          X-Trace: "1"
    - name: run-things
      tool_call_template:
        call_template_type: cli
        url: things
        http_method: GET
holon:
  tools:
    get-things:
      query: {limit: "50"}
"#;

#[test]
fn an_unmodelled_utcp_key_is_ignored_preserved_and_disclosed() {
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(FORWARD).expect("a manual with unmodelled keys still loads");
    let manual = cfg.utcp.clone().expect("the manual parsed");

    let disclosed = manual.unmodelled_keys();
    for expected in [
        "utcp.info",
        "utcp.tools[get-things].auth",
        "utcp.tools[get-things].query_params",
        "utcp.tools[get-things].tool_call_template.headers",
    ] {
        assert!(
            disclosed.iter().any(|k| k == expected),
            "the disclosure must name `{expected}`, got {disclosed:?}"
        );
    }

    // Preserved, not merely tolerated: the value is still readable.
    assert!(
        !manual.extra.is_empty()
            && manual.tools[0]
                .tool_call_template
                .extra
                .contains_key("headers"),
        "an unmodelled key must survive the load so an export can give it back"
    );

    // And the build serves the tool it CAN drive, skipping the one it cannot.
    let built = cfg
        .into_mcp_config_with(
            "things".to_string(),
            &|_: &str| None,
            &holon_mcp_client::credential_path::CredentialRoot::new("/tmp/holon-utcp-forward"),
        )
        .expect("an undrivable tool skips rather than refusing the manual");
    match built.transport {
        holon_mcp_client::mcp_integration::McpTransport::Rest { manual, .. } => {
            assert!(
                manual.calls.contains_key("get-things"),
                "the http tool loads"
            );
            assert!(
                !manual.calls.contains_key("run-things"),
                "the cli tool is skipped, not served"
            );
        }
        other => panic!("expected a utcp connection, got {other:?}"),
    }
}

#[test]
fn an_unmodelled_holon_key_is_still_loudly_refused() {
    let doctored = FORWARD.replace(
        "      query: {limit: \"50\"}",
        "      queryy: {limit: \"50\"}",
    );
    let err = serde_yaml::from_str::<IntegrationFileConfig>(&doctored)
        .expect_err("a typo in OUR half is a mapping that would silently never run");
    let text = err.to_string();
    assert!(
        text.contains("queryy") && text.contains("response"),
        "the refusal must name the typo and list what is accepted, got: {text}"
    );
}

/// Every key a remedy message sends an author to must be a key the parser
/// accepts. Built from the SAME constants the messages interpolate, so the two
/// cannot drift apart again.
#[test]
fn sidecar_remedy_keys_are_keys_the_parser_accepts() {
    use holon_mcp_client::integration_config::CALL_TEMPLATE_KEY;
    use holon_mcp_client::integration_config::HOLON_OAUTH2_KEY;
    use holon_mcp_client::integration_config::HOLON_TOOLS_KEY;
    use holon_mcp_client::integration_config::MANUAL_TOOLS_KEY;

    // The constants say where things live; this sidecar puts them exactly there.
    let (manual_head, manual_tools) = MANUAL_TOOLS_KEY
        .split_once('.')
        .expect("the manual-tools key is dotted");
    let (holon_head, holon_tools) = HOLON_TOOLS_KEY
        .split_once('.')
        .expect("the holon-tools key is dotted");
    let call_template = CALL_TEMPLATE_KEY
        .rsplit_once('.')
        .expect("the call-template key is dotted")
        .1;
    let oauth2 = HOLON_OAUTH2_KEY.split('.').collect::<Vec<_>>();

    let yaml = format!(
        "schema_version: 2
{manual_head}:
  utcp_version: \"1.1.3\"
  manual_version: \"1.0.0\"
  {manual_tools}:
    - name: list-things
      {call_template}:
        call_template_type: http
        url: https://example.invalid/things
        http_method: GET
{holon_head}:
  {}:
    {}:
      auth_url: https://example.invalid/authorize
      token_url: https://example.invalid/token
      client_id_env: THINGS_ID
      client_secret_env: THINGS_SECRET
      refresh_token_file: ${{CONFIG_DIR}}/things-refresh
      scopes: [read]
  {holon_tools}:
    list-things:
      query: {{limit: \"50\"}}
",
        oauth2[1], oauth2[2]
    );

    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).unwrap_or_else(|e| {
        panic!("a remedy message names a key the parser rejects: {e}\n\n{yaml}")
    });
    assert!(
        cfg.utcp.is_some(),
        "{MANUAL_TOOLS_KEY} parsed into a manual"
    );
    assert!(
        cfg.oauth2().is_some(),
        "{HOLON_OAUTH2_KEY} parsed into an OAuth2 block"
    );
    assert!(
        cfg.holon
            .expect("holon section")
            .tools
            .contains_key("list-things"),
        "{HOLON_TOOLS_KEY} parsed into per-tool config"
    );
}
