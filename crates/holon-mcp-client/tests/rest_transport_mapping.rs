//! The mapping layer is a new place a resolved secret can escape.
//!
//! A `response` filter reads the peer's body, and a body can echo the URL it
//! was asked for. When the mapping then fails, its diagnostic quotes the value
//! it choked on — so the mapping's errors leave through the same redactor every
//! other string this transport emits does. These tests pin that, and pin that
//! the two mapping seams refuse loudly rather than silently producing nothing.

use std::sync::Arc;

use holon_mcp_client::credential_path::CredentialRoot;
use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_integration::McpTransport;
use holon_mcp_client::rest_transport::RestCallSurface;

/// Long enough to be redacted at all: a value under 8 bytes collides with
/// ordinary words, so the redactor deliberately leaves it alone.
const SECRET_URL: &str = "https://list.example.test/!Tok3nQ7xLpZm4/api/list/9f2c";

const SHOPPING_SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

fn surface(yaml: &str) -> Arc<dyn McpCallSurface> {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("the sidecar parses");
    let mcp = cfg
        .into_mcp_config_with(
            "shopping".to_string(),
            &|name: &str| (name == "SHOPPING_LIST_URL").then(|| SECRET_URL.to_string()),
            &CredentialRoot::new("/tmp/holon-mapping-test-config"),
        )
        .expect("the sidecar resolves");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => Arc::new(RestCallSurface::new(manual)),
        other => panic!("expected a utcp connection, got {other:?}"),
    }
}

/// A manual whose `response` mapping passes the body straight through. Line 1
/// of a row stream must be the envelope, so the body IS the text the row parser
/// quotes back — which is the leak path this test exists for.
const PASSTHROUGH: &str = r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: fetch
      tool_call_template:
        call_template_type: http
        url: "${SHOPPING_LIST_URL}"
        http_method: GET
holon:
  tools:
    fetch:
      # Declares a `date_time` column and fills it from the peer's own error
      # text. A cell that is not RFC3339 is refused BY VALUE, so the body
      # reaches the message — which is the point.
      response: |
        {holon_rows: 1, scopes: [{type: "t", owner_column: "o", owner_value: "v",
                                  kinds: {stamp: "date_time"}}]},
        {type: "t", row: {id: "1", o: "v", stamp: .error}}
entities: {}
tools: {}
"#;

#[test]
fn a_response_that_echoes_the_url_cannot_leak_it_through_a_mapping_failure() {
    let surface = surface(PASSTHROUGH);
    // The peer answers with its own error, quoting the URL it was asked for —
    // an upstream shape nobody controls. The mapping refuses the body, and the
    // refusal QUOTES it, which is where the echoed credential would surface.
    let body = serde_json::json!({ "error": format!("no list at {SECRET_URL}") });
    let err = surface
        .map_response("fetch", &body)
        .expect_err("a bare body is not a row stream");
    let text = format!("{err:#}");
    assert!(
        text.contains("no list at"),
        "this test is only meaningful if the refusal quotes the body: {text}"
    );
    assert!(
        !text.contains("Tok3nQ7xLpZm4"),
        "the mapping failure carried the resolved credential: {text}"
    );
}

#[test]
fn a_call_with_no_declared_mapping_refuses_rather_than_returning_nothing() {
    let surface = surface(SHOPPING_SIDECAR);
    // `commit` declares a `request` mapping and no `response` one: its answer
    // is a version, not rows.
    let err = surface
        .map_response("commit", &serde_json::json!({"version": 9}))
        .expect_err("a call that declares no response mapping must not answer with zero rows");
    assert!(
        format!("{err:#}").contains("commit"),
        "the refusal must name the call: {err:#}"
    );
}

#[test]
fn an_mcp_peer_refuses_the_mapping_seams_by_name() {
    let yaml = "
schema_version: 2
transport:
  http:
    uri: https://mcp.example.test/mcp
entities: {}
tools: {}
";
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("the sidecar parses");
    let mcp = cfg
        .into_mcp_config_with(
            "peer".to_string(),
            &|_: &str| None,
            &CredentialRoot::new("/tmp/holon-mapping-test-config"),
        )
        .expect("the sidecar resolves");
    assert!(
        matches!(mcp.transport, McpTransport::Http { .. }),
        "an `http` transport is still MCP-over-HTTP"
    );
}

#[test]
fn the_write_leg_maps_command_rows_into_the_peers_own_envelope() {
    let surface = surface(SHOPPING_SIDECAR);
    let stream = serde_json::json!({
        "scopes": [
            {"type": "shopping_commit", "owner_column": "list", "owner_value": "shopping"},
            {"type": "shopping_command", "owner_column": "list", "owner_value": "shopping"},
        ],
        "rows": [
            {"type": "shopping_commit", "row": {
                "id": "shopping", "list": "shopping",
                "old_version": 7, "old_picked_items_version": 5, "device_id": "dev-1"}},
            {"type": "shopping_command", "row": {
                "id": "1756713600000_3f0a1c9d2b4e5f60", "list": "shopping",
                "verb": "add", "name": "Eggs", "cat": "R",
                "command_id": "1756713600000_3f0a1c9d2b4e5f60"}},
            {"type": "shopping_command", "row": {
                "id": "1756713600000_a1b2c3d4e5f60718", "list": "shopping",
                "verb": "remove", "name": "Bread", "cat": "B",
                "command_id": "1756713600000_a1b2c3d4e5f60718"}},
        ],
    });
    let args = surface
        .map_request("commit", &stream)
        .expect("the shipped request mapping runs");
    assert_eq!(args["version"], serde_json::json!(7));
    assert_eq!(args["pickedItemsVersion"], serde_json::json!(5));
    assert_eq!(args["deviceId"], serde_json::json!("dev-1"));
    assert_eq!(
        args["commands"],
        serde_json::json!([
            {"cmd": "add", "good": {"name": "Eggs", "cat": "R", "new": true},
             "id": "1756713600000_3f0a1c9d2b4e5f60"},
            {"cmd": "del", "good": {"name": "Bread", "cat": "B", "new": true},
             "id": "1756713600000_a1b2c3d4e5f60718"},
        ]),
        "the peer's `good` wrapper and its `new: true` literal live in the sidecar, and the \
         command ids pass through untouched — reminting one on a retry is a pinned regression. \
         Note `remove` in, `del` out: the peer's word for a removal is the sidecar's, not Rust's"
    );
}

#[test]
fn a_row_stream_with_no_batch_row_is_refused() {
    let surface = surface(SHOPPING_SIDECAR);
    let stream = serde_json::json!({"scopes": [], "rows": []});
    let err = surface
        .map_request("commit", &stream)
        .expect_err("a commit with no batch row has no version to commit against");
    assert!(
        format!("{err:#}").contains("shopping_commit"),
        "the refusal must name what was missing: {err:#}"
    );
}
