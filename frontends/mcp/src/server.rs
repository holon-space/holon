use holon::api::backend_engine::BackendEngine;
use holon::sync::LoroDocumentStore;
use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, service::RequestContext, tool_handler,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::types::RowChangeJson;

pub struct WatchState {
    pub pending_changes: Arc<Mutex<Vec<RowChangeJson>>>,
    pub _task_handle: JoinHandle<()>,
}

/// Optional services for debug/inspection tools.
/// Fields are Option because Loro and OrgMode may not be enabled.
pub struct DebugServices {
    pub loro_doc_store: Option<Arc<RwLock<LoroDocumentStore>>>,
    pub orgmode_root: Option<PathBuf>,
}

impl Default for DebugServices {
    fn default() -> Self {
        Self {
            loro_doc_store: None,
            orgmode_root: None,
        }
    }
}

pub struct HolonMcpServer {
    pub engine: Arc<BackendEngine>,
    pub debug: Arc<DebugServices>,
    pub watches: Arc<Mutex<HashMap<String, WatchState>>>,
    pub(crate) tool_router: ToolRouter<HolonMcpServer>,
}

impl HolonMcpServer {
    pub fn new(engine: Arc<BackendEngine>, debug: Arc<DebugServices>) -> Self {
        let tool_router = crate::tools::get_tool_router();

        Self {
            engine,
            debug,
            watches: Arc::new(Mutex::new(HashMap::new())),
            tool_router,
        }
    }
}

#[tool_handler]
impl ServerHandler for HolonMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Holon backend engine MCP server for automated testing".into()),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_completions()
                .build(),
            server_info: Implementation::from_build_env(),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        self.list_resources_impl(request, ctx).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        self.read_resource_impl(request, ctx).await
    }

    async fn complete(
        &self,
        _request: CompleteRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        // Return empty completions - we don't provide argument completions yet
        Ok(CompleteResult {
            completion: CompletionInfo {
                values: vec![],
                has_more: Some(false),
                total: Some(0),
            },
            ..Default::default()
        })
    }
}
