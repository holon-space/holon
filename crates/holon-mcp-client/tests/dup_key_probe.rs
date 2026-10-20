//! Probe: nested JSON objects a sidecar declares outside the `utcp:` manual.

use holon_mcp_client::IntegrationFileConfig;

const UTCP_FILE: &str = "\
utcp:
  utcp_version: \"1.0\"
  manual_version: \"1\"
  tools:
    - name: list_items
      tool_call_template:
        call_template_type: http
        url: https://example.invalid/items
        http_method: GET
";

const MCP_FILE: &str = "transport:\n  http:\n    uri: https://example.invalid/mcp\n";

#[test]
fn a_duplicate_key_in_a_declared_json_object_is_a_named_load_error() {
    let cases = [
        (
            format!(
                "{UTCP_FILE}holon:\n  tools:\n    list_items:\n      body: {{ title: \"a\", title: \"b\" }}\n"
            ),
            "holon.tools.body `title`",
        ),
        (
            format!(
                "{MCP_FILE}entities:\n  task:\n    sync:\n      list_tool: list_tasks\n      extract_path: tasks\n      list_params: {{ filter: {{ state: open, state: closed }} }}\n"
            ),
            "sync.list_params nested `state`",
        ),
        (
            format!(
                "{MCP_FILE}entities:\n  task:\n    vtable:\n      search_tool: find_tasks\n      extract_path: tasks\n      static_args: {{ filter: {{ state: open, state: closed }} }}\n"
            ),
            "vtable.static_args nested `state`",
        ),
    ];
    let mut accepted = Vec::new();
    for (yaml, what) in cases {
        match IntegrationFileConfig::from_yaml(&yaml) {
            Ok(_) => accepted.push(format!("{what}: the file loaded")),
            Err(error) => println!("{what}: refused as {error:#}"),
        }
    }
    assert!(
        accepted.is_empty(),
        "a duplicate key must be refused:\n{}",
        accepted.join("\n")
    );
}
