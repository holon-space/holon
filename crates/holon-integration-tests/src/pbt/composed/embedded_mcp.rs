//! The composed `full_headless` session's MCP tools, served by the embedded
//! MCP server over loopback — the prod tool path, not a fake.

use std::sync::Arc;

use holon_mcp::di::EmbeddedMcpServer;

use crate::McpUserDriver;

/// A driver connected to an embedded MCP server over one session's engine.
/// The server stops when this is dropped.
pub struct EmbeddedMcp {
    pub driver: McpUserDriver,
    _server: EmbeddedMcpServer,
}

impl std::ops::Deref for EmbeddedMcp {
    type Target = McpUserDriver;

    fn deref(&self) -> &McpUserDriver {
        &self.driver
    }
}

/// Start the embedded MCP server over a session's engine on a free loopback
/// port, and connect a driver to it. Must run inside the session's runtime.
pub async fn connect_embedded_mcp_parts(
    engine: Arc<holon::api::BackendEngine>,
    services: Arc<dyn holon_frontend::reactive::BuilderServices>,
    debug: Arc<holon_mcp::server::DebugServices>,
    label: &str,
) -> EmbeddedMcp {
    let server =
        holon_mcp::di::start_embedded_mcp_server_on_free_port(Some(engine), Some(services), debug)
            .expect("start the embedded MCP server on a free port");
    // The port is bound before the start returns, so a connection waits in the
    // backlog until the server task accepts it.
    let driver = McpUserDriver::connect(&format!("http://127.0.0.1:{}/mcp", server.port))
        .await
        .unwrap_or_else(|e| panic!("[{label}] connect to the embedded holon MCP server: {e:#}"));
    EmbeddedMcp {
        driver,
        _server: server,
    }
}

/// [`connect_embedded_mcp_parts`] over a booted `full_headless` session.
pub fn connect_embedded_mcp(
    sut: &super::harness::ComposedSut<super::wide_e2e::WideE2E>,
    label: &str,
) -> EmbeddedMcp {
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
    sut.runtime().block_on(async {
        let debug = frontend.mcp_debug_services();
        let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
        connect_embedded_mcp_parts(engine, services, debug, label).await
    })
}

/// `SutDenseTools` for a headless session: the same tool round trips as the
/// live rung, over this session's embedded MCP server.
pub struct EmbeddedDenseTools {
    mcp: EmbeddedMcp,
    resolver: crate::pbt::op_write_cap::IdResolver,
}

impl EmbeddedDenseTools {
    pub fn new(mcp: EmbeddedMcp, resolver: crate::pbt::op_write_cap::IdResolver) -> Self {
        EmbeddedDenseTools { mcp, resolver }
    }

    fn resolve(&self, id: &holon_api::EntityUri) -> holon_api::EntityUri {
        self.resolver
            .lock()
            .expect("resolver lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }
}

#[async_trait::async_trait(?Send)]
impl holon_pbt_core::capabilities::SutDenseTools for EmbeddedDenseTools {
    async fn dense_create_child(
        &self,
        parent: &holon_api::EntityUri,
        place: holon_pbt_core::capabilities::NewRowPlace,
        content: &str,
        tags: &holon_api::Tags,
        properties: &std::collections::BTreeMap<String, String>,
    ) {
        super::live_mcp::dense_create_child_via(
            &self.mcp,
            &self.resolve(parent),
            place,
            content,
            tags,
            properties,
        )
        .await;
    }

    async fn dense_move_first_child_to_end(&self, parent: &holon_api::EntityUri) {
        super::live_mcp::dense_move_first_child_to_end_via(&self.mcp, &self.resolve(parent)).await;
    }

    async fn dense_edit_first_row(
        &self,
        parent: &holon_api::EntityUri,
        title: &str,
        body: Option<&[String]>,
        state: Option<&str>,
    ) {
        super::live_mcp::dense_edit_first_row_via(
            &self.mcp,
            &self.resolve(parent),
            title,
            body,
            state,
        )
        .await;
    }
}

/// The agent's dense tools over an embedded MCP server on `comp`'s engine.
pub async fn embedded_dense_tools(
    comp: &crate::pbt::frontend_slice::components::HeadlessFrontendComponent,
    resolver: &crate::pbt::op_write_cap::IdResolver,
) -> Arc<EmbeddedDenseTools> {
    let started = std::time::Instant::now();
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = comp.reactive();
    let mcp = connect_embedded_mcp_parts(
        comp.engine(),
        services,
        comp.mcp_debug_services(),
        "keystone-dense-tools",
    )
    .await;
    eprintln!(
        "[dense-tools] embedded MCP server up in {:?}",
        started.elapsed()
    );
    Arc::new(EmbeddedDenseTools::new(mcp, resolver.clone()))
}
