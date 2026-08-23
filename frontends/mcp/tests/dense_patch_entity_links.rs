//! `dense_patch` is a WRITE boundary for agent-authored text, so the entity
//! registry has to reach it through the DI container the real servers are built
//! from — not just through the constructor parameter.
//!
//! The parameter existed and every production construction passed `None`, so a
//! `[[<entity>:<id>]]` an agent wrote still degraded to an unknown-scheme link
//! and lost its `block_links` row. These tests pin the WIRING, which is the
//! part that was actually broken.

use std::net::SocketAddr;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_api::link_parser::LinkTarget;
use holon_mcp::di::McpServerConfig;
use holon_mcp::di::McpServerHandle;
use holon_mcp::di::McpServerModule;
use holon_mcp::server::DebugServices;
use holon_mcp::server::HolonMcpServer;
use holon_mcp_client::mcp_sidecar::McpSidecar;
use holon_profiles::TypeRegistry;

/// One multi-word entity — the only shape that can tell a working
/// scheme/table-name join from a broken one.
const SIDECAR_YAML: &str = r#"
entities:
  t_widget:
    id_column: id
    schema:
      - name: id
        sql_type: TEXT
        primary_key: true
"#;

fn registry_with_t_widget() -> Arc<TypeRegistry> {
    let sidecar = McpSidecar::from_yaml(SIDECAR_YAML).expect("sidecar YAML parses");
    let registry = Arc::new(TypeRegistry::new());
    holon_mcp_client::register_sidecar_entity_types(&sidecar, "dense-patch-test", &registry)
        .expect("sidecar entities register");
    registry
}

/// The container that provides `McpServerHandle` must hand it the registry.
/// This is the defect: the provider ignored `TypeRegistry` and passed `None`,
/// so every streamable-HTTP session — the transport embedded agents use — got a
/// registry-less server.
#[test]
fn di_provided_server_handle_carries_the_type_registry() {
    let injector = Injector::root();
    injector.provide::<TypeRegistry>(Provider::root({
        let registry = registry_with_t_widget();
        move |_| Shared::from(registry.clone())
    }));
    injector.provide::<McpServerConfig>(Provider::root(|_| {
        Shared::new(McpServerConfig::with_address(
            "127.0.0.1:0".parse::<SocketAddr>().expect("addr"),
        ))
    }));
    McpServerModule.configure(&injector).expect("configure");

    let handle = injector.resolve::<McpServerHandle>();

    assert!(
        handle.type_registry().is_some(),
        "the DI-provided MCP server handle must carry the live TypeRegistry — without it every \
         session's dense_patch classifies entity links as unknown-scheme"
    );
}

/// The seam `dense_patch` reads. A registry-backed server resolves a sidecar
/// entity; the registry-less one cannot, which is why the wiring above matters.
#[test]
fn a_registry_backed_server_classifies_sidecar_entity_links() {
    let wired = HolonMcpServer::with_type_registry(
        None,
        Some(registry_with_t_widget()),
        Arc::new(DebugServices::default()),
        None,
    );
    assert!(
        matches!(
            wired.link_classifier().classify("t-widget:abc123"),
            LinkTarget::Resolved(_)
        ),
        "a registry-backed server must resolve a YAML-declared entity link"
    );

    let unwired =
        HolonMcpServer::with_type_registry(None, None, Arc::new(DebugServices::default()), None);
    assert!(
        matches!(
            unwired.link_classifier().classify("t-widget:abc123"),
            LinkTarget::UnknownScheme(_)
        ),
        "without a registry the SAME target degrades — this is what the None arm costs"
    );
}
