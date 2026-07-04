//! **Drift guard for the `render_org` MCP tool.**
//!
//! The tool promises "what org write-back would put on disk". This asserts that
//! promise byte-for-byte over the wire: for a settled, seeded document, the
//! tool's `rendered` must equal the bytes write-back actually wrote, read back
//! through `read_org_file`. Any future divergence between the tool's render
//! path and the `FileSyncController`'s fails here.
//!
//! Boots the same composed `full_headless` session the keystone drives, then
//! attaches the embedded `holon` MCP server over it — the same server every
//! frontend launches. Requires `--features pbt`.
//!
//! @pbt kind harness
//! @pbt covers mcp-render-org — the write-back render exposed over MCP

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

type Sut = ComposedSut<WideE2E>;

/// The generator's placeholder document uri: top-level headings are parented to
/// it and `apply_to_ref` remaps them to the resolved per-document uri.
fn gen_placeholder() -> EntityUri {
    EntityUri::block("gen-placeholder")
}

fn heading(id: &str, headline: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), gen_placeholder(), headline);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

/// A page with nesting and a task keyword, so the render exercises headline
/// depth and property drawers rather than a single flat line.
fn page_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: "render_guard.org".to_string(),
        blocks: vec![
            heading("rg-parent", "Parent heading"),
            heading("rg-child-a", "TODO First child"),
            heading("rg-child-b", "Second child"),
        ],
        keyword_set: None,
    }
}

fn step(mut ref_state: ReferenceState, sut: Sut, t: E2ETransition) -> (ReferenceState, Sut) {
    assert!(
        WideE2EMachine::preconditions(&ref_state, &t),
        "preconditions failed for {t:?}"
    );
    ref_state = WideE2EMachine::apply(ref_state, &t);
    let sut = <Sut as StateMachineTest>::apply(sut, &ref_state, t);
    (ref_state, sut)
}

/// Bind an ephemeral loopback port, then release it — the embedded MCP server
/// re-binds it. A tiny TOCTOU window, acceptable for a single-process test.
fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

#[test]
fn mcp_render_org_matches_writeback() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);
    let (ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(page_file()));

    let doc_uri = ref_state
        .doc_uri_by_name("render_guard")
        .expect("render_guard page block must exist after WriteOrgFile");
    sut.settle_projections();

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
        let services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices> =
            reactive.clone();
        holon_mcp::di::start_embedded_mcp_server_with_debug(
            Some(engine.clone()),
            Some(services),
            port,
            debug,
        );
    }
    holon_integration_tests::pbt::wait_for_mcp_listener(
        port,
        holon_integration_tests::pbt::ui_harness::MCP_BIND_TIMEOUT,
        "mcp-render-org-writeback",
    );

    let base_url = format!("http://127.0.0.1:{port}/mcp");
    let driver = sut
        .runtime()
        .block_on(McpUserDriver::connect(&base_url))
        .expect("connect to the embedded holon MCP server");

    // Address the document the way an agent does: by the alias
    // `list_loro_documents` publishes for its file.
    let docs = sut.runtime().block_on(async {
        driver
            .call_tool_json("list_loro_documents", serde_json::json!({}))
            .await
            .expect("list_loro_documents over MCP")
    });
    let doc_id = docs["aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("list_loro_documents response missing `aliases`: {docs}"))
        .iter()
        .find(|a| {
            a["file_path"]
                .as_str()
                .is_some_and(|p| p.ends_with("render_guard.org"))
        })
        .map(|a| a["alias"].as_str().expect("alias is a string").to_string())
        .unwrap_or_else(|| panic!("no alias for render_guard.org: {docs}"));

    let render = |source: &str, scope: &str| {
        let args = serde_json::json!({ "doc_id": doc_id, "source": source, "scope": scope });
        let value = sut.runtime().block_on(async {
            driver
                .call_tool_json("render_org", args)
                .await
                .unwrap_or_else(|e| panic!("render_org({source}, {scope}) over MCP failed: {e:#}"))
        });
        value["rendered"]
            .as_str()
            .unwrap_or_else(|| panic!("render_org({source}, {scope}) missing `rendered`: {value}"))
            .to_string()
    };

    let rendered = sut.runtime().block_on(async {
        driver
            .call_tool_json(
                "render_org",
                serde_json::json!({ "doc_id": doc_id, "source": "sql", "scope": "document" }),
            )
            .await
            .expect("render_org over MCP")
    });
    let disk = sut.runtime().block_on(async {
        driver
            .call_tool_json("read_org_file", serde_json::json!({ "doc_id": doc_id }))
            .await
            .expect("read_org_file over MCP")
    });

    let rendered_text = rendered["rendered"]
        .as_str()
        .unwrap_or_else(|| panic!("render_org response missing `rendered`: {rendered}"));
    let disk_text = disk["content"]
        .as_str()
        .unwrap_or_else(|| panic!("read_org_file response missing `content`: {disk}"));

    assert!(
        rendered_text.contains("Parent heading"),
        "render_org returned no seeded content — the tool is not reading the settled \
         document. rendered={rendered_text:?}"
    );
    assert_eq!(
        rendered_text, disk_text,
        "render_org(sql, document) must reproduce the bytes org write-back wrote for {doc_uri} — \
         the tool's render path has drifted from the FileSyncController's"
    );

    // ── The other three points of the source × scope surface. ──
    let sql_blocks = render("sql", "blocks");
    assert!(
        !sql_blocks.starts_with("#+") && sql_blocks.contains("Parent heading"),
        "scope=blocks must drop the document header and keep the body. rendered={sql_blocks:?}"
    );
    assert!(
        rendered_text.contains(sql_blocks.trim()),
        "scope=document must be scope=blocks plus a header. document={rendered_text:?} \
         blocks={sql_blocks:?}"
    );

    let loro_blocks = render("loro", "blocks");
    assert_eq!(
        loro_blocks, sql_blocks,
        "at quiescence the Loro tree and the SQL write authority hold the same body — a \
         difference here is the Loro↔SQL divergence this tool exists to surface"
    );

    let loro_document = render("loro", "document");
    assert_eq!(
        loro_document, rendered_text,
        "at quiescence source=loro and source=sql agree at scope=document too — the Loro header \
         block must render like the write authority's"
    );

    // An unparseable axis value is rejected at the boundary, not silently
    // degraded to a default render.
    let bad = sut.runtime().block_on(async {
        driver
            .call_tool_json(
                "render_org",
                serde_json::json!({ "doc_id": doc_id, "source": "sqlite" }),
            )
            .await
    });
    assert!(
        bad.is_err(),
        "render_org must reject an unknown `source`, not fall back to a default: {bad:?}"
    );
}
