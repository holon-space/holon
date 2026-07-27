//! **Wire-contract guard for the `list_keybindings` MCP tool.**
//!
//! The tool promises the LIVE keybinding registry. This reconstructs that map
//! from the wire and asserts it equals the running `ReactiveEngine`'s own
//! registry — pinning the shape, the completeness, and the key-name spelling.
//! It also pins the property the live-MCP keystone depends on: a chord read
//! here is spelled the way `send_key_chord` takes it, so a binding can be sent
//! straight back instead of hardcoded.
//!
//! What it deliberately cannot prove: that the tool reads the registry the app
//! actually uses. Both here and in the GPUI frontend the MCP server is handed
//! the very same `ReactiveEngine` the UI renders from, so "a second registry"
//! is unrepresentable rather than untested.
//!
//! Boots the same composed `full_headless` session the keystone drives, then
//! attaches the embedded `holon` MCP server over it. Requires `--features pbt`.
//!
//! @pbt kind harness
//! @pbt covers mcp-list-keybindings — the chord registry exposed over MCP

use std::collections::BTreeMap;

use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::reactive::BuilderServices;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use proptest_state_machine::StateMachineTest;

type Sut = ComposedSut<WideE2E>;

/// Bind an ephemeral loopback port, then release it — the embedded MCP server
/// re-binds it. A tiny TOCTOU window, acceptable for a single-process test.
fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

#[test]
fn mcp_list_keybindings_matches_registry() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);

    let engine = sut
        .handle()
        .engine()
        .expect("full_headless boots a Turso engine")
        .clone();
    let reactive = sut
        .handle()
        .reactive()
        .expect("full_headless boots a ReactiveEngine");
    let frontend = sut
        .handle()
        .frontend()
        .expect("full_headless boots a frontend component")
        .clone();

    let port = free_loopback_port();
    // The embedded server reads MCP_SERVER_PORT (default_port is only a
    // fallback) — pin it so the listener lands where the driver connects.
    unsafe {
        std::env::set_var("MCP_SERVER_PORT", port.to_string());
    }
    let debug = sut.runtime().block_on(frontend.mcp_debug_services());
    {
        let _guard = sut.runtime().enter();
        let services: std::sync::Arc<dyn BuilderServices> = reactive.clone();
        holon_mcp::di::start_embedded_mcp_server_with_debug(
            Some(engine.clone()),
            Some(services),
            port,
            debug,
        );
    }
    std::thread::sleep(std::time::Duration::from_secs(2));

    let base_url = format!("http://127.0.0.1:{port}/mcp");
    let driver = sut
        .runtime()
        .block_on(McpUserDriver::connect(&base_url))
        .expect("connect to the embedded holon MCP server");

    let response = sut.runtime().block_on(async {
        driver
            .call_tool_json("list_keybindings", serde_json::json!({}))
            .await
            .expect("list_keybindings over MCP")
    });

    let bindings = response["bindings"]
        .as_array()
        .unwrap_or_else(|| panic!("list_keybindings response missing `bindings`: {response}"));

    // Rebuild the map from the wire, parsing each key name back through the
    // one shared vocabulary — a name the client cannot parse is a wire-format
    // regression, not a soft warning.
    let over_the_wire: BTreeMap<String, KeyChord> = bindings
        .iter()
        .map(|entry| {
            let action = entry["action"]
                .as_str()
                .unwrap_or_else(|| panic!("binding {entry} has no string `action`"))
                .to_string();
            let keys = entry["chord"]
                .as_array()
                .unwrap_or_else(|| panic!("binding {entry} has no `chord` array"));
            let chord = KeyChord(
                keys.iter()
                    .map(|k| {
                        k.as_str()
                            .unwrap_or_else(|| panic!("chord key {k} is not a string"))
                            .parse::<Key>()
                            .unwrap_or_else(|e| {
                                panic!("chord key {k} of `{action}` is unparseable: {e}")
                            })
                    })
                    .collect(),
            );
            (action, chord)
        })
        .collect();

    let in_process = reactive.key_bindings_snapshot();

    assert_eq!(
        over_the_wire, in_process,
        "list_keybindings must report the running registry verbatim — the tool has drifted from \
         ReactiveEngine::key_bindings"
    );

    // The binding the live-MCP keystone's reorder caps look up. Its VALUE is
    // deliberately not asserted: the point of the tool is that the test stops
    // caring what the chord is.
    for action in ["move_up", "move_down"] {
        assert!(
            over_the_wire.contains_key(action),
            "the reorder action `{action}` must be bound — the live-MCP keystone's \
             MoveBlockUp/MoveBlockDown caps resolve their chord through it. bound actions: {:?}",
            over_the_wire.keys().collect::<Vec<_>>()
        );
    }

    // A chord read here must be spelled the way send_key_chord takes it, or
    // "read the binding instead of hardcoding it" is not actually available.
    // Only the key-NAME vocabulary is under test: whether this particular
    // entity has a matching bound operation is a different question, so any
    // outcome except an unparseable key passes.
    let move_up: Vec<String> = over_the_wire["move_up"]
        .0
        .iter()
        .map(|k| k.to_string())
        .collect();
    let outcome = sut.runtime().block_on(async {
        driver
            .call_tool_json(
                "send_key_chord",
                serde_json::json!({
                    "entity_id": holon_api::root_layout_block_uri().to_string(),
                    "keys": move_up,
                }),
            )
            .await
    });
    if let Err(e) = &outcome {
        let message = format!("{e:#}");
        assert!(
            !message.contains("Unknown key"),
            "send_key_chord could not parse the key names list_keybindings emitted \
             ({move_up:?}) — the two tools disagree on the key vocabulary: {message}"
        );
    }
}
