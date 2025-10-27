use holon::api::backend_engine::BackendEngine;
use holon::api::holon_service::HolonService;
use holon::sync::LoroDocumentStore;
use holon_frontend::focus_path::InputRouter;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
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

/// A command sent from an MCP tool (tokio thread) to the GPUI foreground
/// thread for dispatch as a platform input event.
pub struct InteractionCommand {
    pub event: InteractionEvent,
    pub response_tx: tokio::sync::oneshot::Sender<InteractionResponse>,
}

/// Raw input events that the MCP server can inject into the GPUI window.
///
/// `MouseClick` is a fused Down+Up at the same coordinate (no movement
/// between). For anything that depends on the press-hold-release shape
/// of the gesture — drag&drop, click-and-hold context menus, slider
/// scrubbing, multi-step pointer sequences — use `MouseDown` /
/// `MouseUp` separately and emit `MouseMove` events with
/// `pressed_button = Some("left")` between them.
pub enum InteractionEvent {
    MouseClick {
        position: (f32, f32),
        button: String,
        modifiers: Vec<String>,
    },
    /// Press a mouse button without releasing. Used by drag&drop to keep a
    /// pointer captured while subsequent `MouseMove` events fire.
    MouseDown {
        position: (f32, f32),
        button: String,
        modifiers: Vec<String>,
    },
    /// Release a mouse button at a position. Pairs with `MouseDown` to
    /// complete a drag gesture; GPUI's drop handlers fire on this event.
    MouseUp {
        position: (f32, f32),
        button: String,
        modifiers: Vec<String>,
    },
    KeyDown {
        keystroke: String,
        modifiers: Vec<String>,
    },
    KeyUp {
        keystroke: String,
        modifiers: Vec<String>,
    },
    /// Move the pointer. `pressed_button` mirrors GPUI's `MouseMoveEvent` —
    /// when set, GPUI treats this as a drag move (which is required for
    /// `cx.active_drag` to populate after a `MouseDown` on a draggable).
    MouseMove {
        position: (f32, f32),
        #[allow(dead_code)]
        pressed_button: Option<String>,
        #[allow(dead_code)]
        modifiers: Vec<String>,
    },
    /// Turn the scroll wheel at a window position. `delta` is line-based
    /// (positive `dy` = down, positive `dx` = right).
    ScrollWheel {
        position: (f32, f32),
        delta: (f32, f32),
        modifiers: Vec<String>,
    },
}

/// Result of dispatching an interaction event through the GPUI window.
pub struct InteractionResponse {
    pub handled: bool,
    pub detail: Option<String>,
}

/// Optional services for debug/inspection tools.
/// Fields use `OnceLock` so they can be populated after DI resolution
/// (e.g. Loro doc store is only available after `FrontendSession` is created).
pub struct DebugServices {
    pub loro_doc_store: std::sync::OnceLock<Arc<RwLock<LoroDocumentStore>>>,
    pub orgmode_root: std::sync::OnceLock<PathBuf>,
    /// Shared navigation debug state. Written by the GPUI frontend on each
    /// render, read by the `describe_navigation` MCP tool.
    /// Uses std::sync::RwLock (not tokio) since GPUI writes from sync context.
    pub navigation_state: Arc<std::sync::RwLock<NavigationDebugState>>,
    /// Shared input router for semantic UI interaction (navigation, key chords).
    /// Set by the GPUI frontend; MCP tools call `bubble_input` on it.
    pub input_router: Arc<InputRouter>,
    /// Channel for injecting raw input events into the GPUI window.
    /// Set by the GPUI frontend after window creation.
    /// Uses `futures::channel::mpsc` so the pump awaits messages instead of
    /// polling at 16ms — eliminates executor starvation during heavy workloads.
    pub interaction_tx: std::sync::OnceLock<futures::channel::mpsc::Sender<InteractionCommand>>,
    /// Entity ID of the currently focused editor element.
    /// Written by GPUI on every focus change (cross-block nav, mouse click).
    /// Read by PBT to verify navigation results.
    pub focused_element_id: Arc<std::sync::RwLock<Option<String>>>,
    /// Frontend-supplied `UserDriver` for dispatching real UI mutations
    /// through the same channel used by click/key/scroll MCP tools.
    /// The GPUI frontend installs a channel-based driver here after
    /// window creation; MCP tools read it to stay decoupled from the
    /// concrete frontend.
    pub user_driver: std::sync::OnceLock<Arc<dyn UserDriver>>,
}

/// Snapshot of cross-block navigation state for MCP inspection.
pub struct NavigationDebugState {
    /// Reactive tree dump (from InputRouter::describe).
    pub tree_description: String,
    /// Editor input row_ids.
    pub editor_input_ids: Vec<String>,
    /// Entity view registries dump (from EntityViewRegistries::describe).
    pub entity_registry_description: String,
}

impl Default for NavigationDebugState {
    fn default() -> Self {
        Self {
            tree_description: "(not yet built)".to_string(),
            editor_input_ids: Vec::new(),
            entity_registry_description: "(not yet populated)".to_string(),
        }
    }
}

impl Default for DebugServices {
    fn default() -> Self {
        Self {
            loro_doc_store: std::sync::OnceLock::new(),
            orgmode_root: std::sync::OnceLock::new(),
            navigation_state: Arc::new(std::sync::RwLock::new(NavigationDebugState::default())),
            input_router: Arc::new(InputRouter::new()),
            interaction_tx: std::sync::OnceLock::new(),
            focused_element_id: Arc::new(std::sync::RwLock::new(None)),
            user_driver: std::sync::OnceLock::new(),
        }
    }
}

pub struct HolonMcpServer {
    pub engine: Option<Arc<BackendEngine>>,
    pub service: Option<HolonService>,
    pub type_registry: Option<Arc<holon::type_registry::TypeRegistry>>,
    pub debug: Arc<DebugServices>,
    pub builder_services: Option<Arc<dyn BuilderServices>>,
    pub watches: Arc<Mutex<HashMap<String, WatchState>>>,
    pub(crate) tool_router: ToolRouter<HolonMcpServer>,
}

impl HolonMcpServer {
    pub fn new(
        engine: Option<Arc<BackendEngine>>,
        debug: Arc<DebugServices>,
        builder_services: Option<Arc<dyn BuilderServices>>,
    ) -> Self {
        Self::with_type_registry(engine, None, debug, builder_services)
    }

    pub fn with_type_registry(
        engine: Option<Arc<BackendEngine>>,
        type_registry: Option<Arc<holon::type_registry::TypeRegistry>>,
        debug: Arc<DebugServices>,
        builder_services: Option<Arc<dyn BuilderServices>>,
    ) -> Self {
        let tool_router = if engine.is_some() {
            Self::tool_router_ui() + Self::tool_router_backend()
        } else {
            Self::tool_router_ui()
        };

        let service = engine.as_ref().map(|e| HolonService::new(e.clone()));

        Self {
            engine,
            service,
            type_registry,
            debug,
            builder_services,
            watches: Arc::new(Mutex::new(HashMap::new())),
            tool_router,
        }
    }

    /// Access the backend engine. Panics if not available.
    ///
    /// Safe because backend tools are only registered when engine is `Some`.
    /// If this panics, a backend tool was somehow called in design gallery mode.
    pub(crate) fn engine(&self) -> &Arc<BackendEngine> {
        self.engine.as_ref().expect(
            "BackendEngine accessed but not available — \
             backend tools should not be registered in design gallery mode",
        )
    }

    /// Access the shared service layer. Panics if not available.
    pub(crate) fn service(&self) -> &HolonService {
        self.service.as_ref().expect(
            "HolonService accessed but not available — \
             backend tools should not be registered in design gallery mode",
        )
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
