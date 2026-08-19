use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::api::holon_service::HolonService;
use holon_core::storage::BlockQuerySource;
use holon_frontend::focus_path::InputRouter;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroSyncControllerHandle;
use holon_orgmode::OrgSyncIdleSignal;
use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::tool_handler;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::types::RowChangeJson;

pub struct WatchState {
    pub pending_changes: Arc<Mutex<Vec<RowChangeJson>>>,
    pub task_handle: JoinHandle<()>,
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
#[derive(Debug)]
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
    /// Insert text the way a soft keyboard's `insertText:` delivers it —
    /// as a finished string, NOT as a sequence of hardware `KeyDown`s.
    ///
    /// This is the harness-faithful mirror of the iOS UIKit text-input path
    /// (`gpui-mobile`'s `IosWindow::handle_text_input`): the GPUI keymap is
    /// bypassed and the text is committed straight into the focused editor's
    /// input handler. A soft `Return` arrives here as `"\n"` and must become
    /// an `enter` action (split_block), not a literal newline — see the
    /// GPUI-side handler. `KeyDown` (used by `type_text`) cannot reach this
    /// path, which is why a soft-keyboard-only bug can escape a KeyDown-driven
    /// test.
    InsertText { text: String },
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
    /// Scroll a virtualized list (e.g. the LeftSidebar) so the named entity
    /// is in the viewport, then notify and flush. This is NOT a raw
    /// platform-input event — the GPUI handler looks up the relevant
    /// `ReactiveShell::list_state_handle()` and calls
    /// `scroll_to_reveal_item(ix)`. Block-mode panels (Main) need no
    /// scroll: every rendered entity already has bounds recorded during
    /// prepaint, viewport or not. Used by `wait_for_entity_bounds` after
    /// a short polling window when bounds haven't appeared yet.
    ScrollEntityIntoView { entity_id: String },
    /// Scroll a virtualized list by a pixel delta, driving the target panel's
    /// `ListState::scroll_by` DIRECTLY (like `ScrollEntityIntoView`) rather
    /// than synthesizing a `ScrollWheel` platform input. A synthetic
    /// `ScrollWheelEvent` dispatched off-cursor does not satisfy gpui's
    /// `Hitbox::should_handle_scroll` hover gate, so it silently no-ops even
    /// though the list is scrollable (dogfood #3: the tool reported success
    /// while nothing moved). `entity_id` names either a panel
    /// (`block:default-*`, scrolls its primary list) or a block inside one
    /// (scrolls the list that contains it). `dy` is a pixel delta
    /// (positive = down / toward the end); `dx` is reserved. The GPUI handler
    /// reports `handled=false` when no scrollable list is reachable, so the
    /// driver can fail loud instead of faking success.
    ScrollList { entity_id: String, dx: f32, dy: f32 },
    /// Capture the window's last rendered frame as an RGBA image via the
    /// platform's `render_to_image` (offscreen wgpu readback on Android). Backs
    /// the `screenshot` MCP tool on platforms with no OS-level window-capture
    /// path. The pump answers with [`InteractionResponse::screenshot`] set.
    CaptureScreenshot,
    /// Draw one frame, now, on the main thread — no platform input, no visual
    /// side effect.
    ///
    /// Render-derived state (the `BoundsRegistry`'s rects, and the
    /// `editable_text` window-focus flag it carries) is only as fresh as the
    /// last frame that painted. A window that is not the frontmost application
    /// paints when the platform decides to, so a driver waiting on that state
    /// can sit out its whole budget while the thing it waits for has already
    /// happened — reporting a failure that is purely an artifact of nobody
    /// having drawn (dogfood 2026-08-07, DRIVER PARITY). `window.refresh()`
    /// only marks the window dirty; this forces the draw itself, so a wait
    /// paces on frames it produces rather than frames it hopes for.
    ForceFrame,
}

/// Raw pixel readback of the GPUI window, produced by
/// [`InteractionEvent::CaptureScreenshot`]. `rgba` is tightly packed
/// `width * height * 4` bytes; the MCP `screenshot` tool PNG-encodes it.
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Result of dispatching an interaction event through the GPUI window.
pub struct InteractionResponse {
    pub handled: bool,
    pub detail: Option<String>,
    /// Present only for [`InteractionEvent::CaptureScreenshot`]; `None` for
    /// every input event.
    pub screenshot: Option<CapturedImage>,
}

/// One window-level key binding, as the frontend registered it with the
/// platform keymap.
///
/// The structural registry (`BuilderServices::key_bindings_snapshot`) only
/// knows chords wired to reactive operations; window chords (undo, redo,
/// quick-open, tab switching) live in the platform keymap and were invisible
/// to `list_keybindings` until this carried them across (dogfood 2026-08-07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowKeyBinding {
    /// Action name, in the same namespace `list_keybindings` reports.
    pub action: String,
    /// Chord keys in the `holon_api::Key` wire vocabulary, e.g.
    /// `["cmd", "shift", "z"]` — directly re-sendable via `send_key_chord`.
    pub keys: Vec<String>,
    /// The keymap context the binding is scoped to (`None` = global). A
    /// context-scoped chord only fires while that context is active, which an
    /// agent reading the registry must be able to see.
    pub context: Option<String>,
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
    /// Shared input router for semantic UI interaction (navigation, key
    /// chords). Set by the GPUI frontend; MCP tools call `bubble_input` on
    /// it.
    pub input_router: Arc<InputRouter>,
    /// Window-level key bindings the frontend registered with the platform
    /// keymap. `list_keybindings` unions these with the structural registry;
    /// unset in a headless run, where the tool then reports the structural
    /// registry alone and says so.
    pub window_key_bindings: std::sync::OnceLock<Vec<WindowKeyBinding>>,
    /// Channel for injecting raw input events into the GPUI window.
    /// Set by the GPUI frontend after window creation.
    /// Uses `futures::channel::mpsc` so the pump awaits messages instead of
    /// polling at 16ms — eliminates executor starvation during heavy workloads.
    pub interaction_tx: std::sync::OnceLock<futures::channel::mpsc::Sender<InteractionCommand>>,
    /// Frontend-supplied `UserDriver` for dispatching real UI mutations
    /// through the same channel used by click/key/scroll MCP tools.
    /// The GPUI frontend installs a channel-based driver here after
    /// window creation; MCP tools read it to stay decoupled from the
    /// concrete frontend.
    pub user_driver: std::sync::OnceLock<Arc<dyn UserDriver>>,
    /// Frontend-supplied measured-layout registry. `describe_ui` joins its
    /// nodes against this to report the rects the window actually painted.
    /// Unset in a headless run — the tool then declares geometry unavailable
    /// rather than reporting zeros.
    pub geometry: std::sync::OnceLock<Arc<dyn holon_frontend::geometry::GeometryProvider>>,
    /// FileSystem port for org-file reads (ADR 0011). Populated from DI by
    /// `DebugServicesPopulatorModule` so inspection tools see the same vault
    /// the session uses (in tests: the in-memory filesystem).
    pub org_fs: std::sync::OnceLock<Arc<dyn holon_filesystem::FileSystem>>,
    /// Channel to the GPUI main-thread reset pump (Phase 1 Option A). The
    /// `reset_vault` tool (tokio) sends a [`ResetRequest`] carrying a freshly
    /// built session+engine; the pump owns the `!Send` `RebindHandle` and
    /// re-points the live window onto them. Installed by the frontend after
    /// window creation. `None` when the frontend didn't wire a reset pump.
    pub reset_tx: std::sync::OnceLock<futures::channel::mpsc::Sender<ResetRequest>>,
    /// Frontend-supplied builder that boots a fresh, seeded SUT WITHOUT
    /// starting a second MCP server (the existing one is reused via the
    /// swappable [`LiveMcpBackend`] cell). Lives in the gpui crate;
    /// installed here so the `reset_vault` tool stays decoupled from gpui.
    /// Runs on the same tokio runtime the MCP server runs on.
    pub reset_builder: std::sync::OnceLock<ResetBuilderFn>,
    /// Live, swappable convergence/mirror handles the debug PBT tools
    /// (`await_quiescence`, `debug_pbt_snapshot`) read. Populated at boot by
    /// the frontend and SWAPPED in place by `reset_vault` — so a per-case
    /// rebind points these at the fresh session's Loro sync controller /
    /// org idle signal / CDC-driven `BlockQuerySource` instead of the
    /// retired one. A plain boot-time `OnceLock` (like
    /// [`Self::loro_doc_store`]) would go stale after a reset and silently
    /// answer against the retired engine; this cell fails that failure mode
    /// by being swappable alongside [`LiveMcpBackend`].
    pub live_debug: LiveDebugHandles,
    /// The class-1 invariant suite `run_self_checks` dispatches to. Registered
    /// once at boot by the frontend; only a `pbt`-featured build carries an
    /// implementation, so `None` is a build fact the tool reports as an error
    /// naming the fix, never as an empty pass. A boot-time `OnceLock` is
    /// reset-safe here (unlike [`Self::live_debug`]) because the suite reads
    /// the handles it is HANDED per call, holding none of its own.
    pub self_check_suite: std::sync::OnceLock<Arc<dyn crate::self_check::LiveSelfCheckSuite>>,
    /// Serializes `send_key_chord` presses across every session's server.
    /// The tool attributes what a chord DID by reading the dispatch journal
    /// between a mark and the press returning; two presses in flight at once
    /// would each claim the other's entries.
    pub key_chord_press: tokio::sync::Mutex<()>,
}

/// Live, swappable [`DebugHandlesCell`] shared across every session's server (a
/// cloned `Arc`), so a `reset_vault` swap is visible everywhere.
/// `std::sync::RwLock` (not tokio's) so the debug tools can read it without an
/// `.await`.
pub type LiveDebugHandles = Arc<std::sync::RwLock<DebugHandlesCell>>;

/// The swappable convergence/mirror handles behind
/// [`DebugServices::live_debug`]. Each is `None` when the running config
/// doesn't wire that substrate (Loro off, no org file-sync). The debug tools
/// distinguish "not wired" from "should be wired but unreachable" and fail loud
/// on the latter rather than skipping.
#[derive(Default, Clone)]
pub struct DebugHandlesCell {
    /// The frontend's Loro sync controller — `last_synced_frontiers()` is the
    /// Loro convergence watermark; `error_count()` backs `loro_had_errors`.
    pub loro_sync_handle: Option<Arc<LoroSyncControllerHandle>>,
    /// The file-sync controller's idle signal — `current_tick()` bumps after
    /// every processed change, the org convergence signal.
    pub org_idle_signal: Option<Arc<OrgSyncIdleSignal>>,
    /// The CDC-driven `LiveData<Block>`/`LiveData<FocusRoot>` mirror producer —
    /// `snapshot()` captures the live mirrors (NOT a matview SQL read).
    pub block_query_source: Option<Arc<dyn BlockQuerySource>>,
    /// The live Loro document store — its global doc backs `lamport_height`,
    /// `loro_tree_children`, and the current-frontier side of the Loro signal.
    /// A reset-safe copy of [`DebugServices::loro_doc_store`] that IS swapped.
    pub loro_doc_store: Option<Arc<RwLock<LoroDocumentStore>>>,
    /// The reactive engine — `focused_block()` is the engine's authoritative
    /// focus, exposed via `debug_pbt_snapshot` so the live-MCP PBT driver can
    /// wait for a click's focus to land before dispatching caret keystrokes.
    pub reactive_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// The org write-back renderer — backs `render_org`. `None` when the
    /// running config wires no file sync.
    pub writeback_renderer: Option<Arc<holon_filesystem::WritebackRenderer>>,
}

impl DebugServices {
    /// The session's org filesystem. When no DI binding was populated
    /// (standalone usage without orgmode), reads the real disk — identical
    /// to the pre-port behaviour; that degraded mode is the disclosed
    /// default here, not a hidden swallow.
    pub fn org_filesystem(&self) -> Arc<dyn holon_filesystem::FileSystem> {
        self.org_fs
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(holon_filesystem::RealFileSystem))
    }
}

/// Snapshot of cross-block navigation state for MCP inspection.
pub struct NavigationDebugState {
    /// Reactive tree dump (from InputRouter::describe).
    pub tree_description: String,
}

impl Default for NavigationDebugState {
    fn default() -> Self {
        Self {
            tree_description: "(not yet built)".to_string(),
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
            window_key_bindings: std::sync::OnceLock::new(),
            interaction_tx: std::sync::OnceLock::new(),
            user_driver: std::sync::OnceLock::new(),
            geometry: std::sync::OnceLock::new(),
            org_fs: std::sync::OnceLock::new(),
            reset_tx: std::sync::OnceLock::new(),
            reset_builder: std::sync::OnceLock::new(),
            live_debug: Arc::new(std::sync::RwLock::new(DebugHandlesCell::default())),
            self_check_suite: std::sync::OnceLock::new(),
            key_chord_press: tokio::sync::Mutex::new(()),
        }
    }
}

/// A request from the `reset_vault` tool (tokio) to the GPUI main-thread reset
/// pump: re-point the live window onto this freshly-built session+engine. The
/// pump owns the `!Send` `RebindHandle`, so both Arcs (which already cross the
/// tokio↔GPUI boundary at boot) are handed across and the pump does the
/// main-thread `rebind`. `ack` reports the rebind result back to the tool.
pub struct ResetRequest {
    pub session: Arc<holon_frontend::FrontendSession>,
    pub engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    pub ack: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
}

/// What the gpui-side reset builder returns to the `reset_vault` tool. All
/// members are `Send` so the tool can push them onto its retirement list.
pub struct ResetBuildOutput {
    pub session: Arc<holon_frontend::FrontendSession>,
    pub engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    pub backend: Arc<BackendEngine>,
    /// Opaque holder keeping the fresh SUT's temp dirs (org root, db, config)
    /// alive. The tool moves it onto the retirement list so the retired
    /// engine's watchers/consolidator idle against still-existing but
    /// abandoned paths (plan F: leak deliberately, isolate completely).
    pub retire: Box<dyn std::any::Any + Send>,
    /// Fresh convergence/mirror handles resolved from the reset's own DI
    /// injector — `reset_vault` swaps these into [`DebugServices::live_debug`]
    /// so the debug PBT tools read the fresh session, not the retired one.
    pub live_debug: DebugHandlesCell,
}

/// Frontend-installed reset builder: takes the seed files (`(name, content)`)
/// and boots a fresh, seeded SUT on fresh temp paths. Boxed future so the
/// gpui-crate async builder can be stored on the mcp-crate `DebugServices`.
pub type ResetBuilderFn = Arc<
    dyn Fn(
            Vec<(String, String)>,
        ) -> futures::future::BoxFuture<'static, anyhow::Result<ResetBuildOutput>>
        + Send
        + Sync,
>;

/// Live, swappable backend the MCP tools read **per call**. A per-case
/// `reset_vault` rebind (Phase 1 Option A) swaps this cell in place, so EVERY
/// subsequent tool call — even on a streamable-http session opened *before* the
/// reset — observes the FRESH engine instead of the retired one (plan C2).
/// Mirrors the window's `LiveEngine` cell. `std::sync::RwLock` (not tokio's) so
/// the sync tool accessors can read it without an `.await`.
pub type LiveMcpBackend = Arc<std::sync::RwLock<McpBackendCell>>;

/// The swappable contents of a [`LiveMcpBackend`].
#[derive(Default, Clone)]
pub struct McpBackendCell {
    pub engine: Option<Arc<BackendEngine>>,
    pub builder_services: Option<Arc<dyn BuilderServices>>,
}

pub struct HolonMcpServer {
    /// The live, swappable backend (engine + builder services). Shared across
    /// every session's server via a cloned `Arc`, so a reset swap is visible
    /// everywhere.
    pub backend: LiveMcpBackend,
    pub type_registry: Option<Arc<holon_profiles::TypeRegistry>>,
    pub debug: Arc<DebugServices>,
    pub watches: Arc<Mutex<HashMap<String, WatchState>>>,
    pub(crate) tool_router: ToolRouter<HolonMcpServer>,
    /// Handle → dense projection store for the `dense_query`/`dense_patch` pair
    /// (in-memory, TTL-evicted). Shared across sessions like `watches`.
    pub(crate) dense_projections: crate::dense_projection::ProjectionRegistry,
    /// Stable identity of this MCP server instance — the agent-session id
    /// stamped as provenance on every op the agent drives (C2a supervision).
    session_id: String,
}

impl HolonMcpServer {
    /// The classifier every parse boundary this server owns must use.
    ///
    /// Registry-backed when one was wired, so `[[<entity>:<id>]]` resolves for
    /// exactly the entities that exist. The `None` arm knows only the built-in
    /// schemes — every sidecar entity link degrades to unknown-scheme and loses
    /// its `block_links` row — so it exists only for servers built without a
    /// container at all.
    pub fn link_classifier(&self) -> holon_api::link_parser::LinkTargetClassifier {
        match &self.type_registry {
            Some(registry) => registry.link_target_classifier(),
            None => holon_api::link_parser::LinkTargetClassifier::default(),
        }
    }

    pub fn with_type_registry(
        engine: Option<Arc<BackendEngine>>,
        type_registry: Option<Arc<holon_profiles::TypeRegistry>>,
        debug: Arc<DebugServices>,
        builder_services: Option<Arc<dyn BuilderServices>>,
    ) -> Self {
        let backend = Arc::new(std::sync::RwLock::new(McpBackendCell {
            engine,
            builder_services,
        }));
        Self::with_backend_cell(backend, type_registry, debug)
    }

    /// Build a server that shares an existing [`LiveMcpBackend`] cell — the
    /// session factory uses this so all sessions (and the reset tool) point at
    /// ONE swappable cell.
    pub fn with_backend_cell(
        backend: LiveMcpBackend,
        type_registry: Option<Arc<holon_profiles::TypeRegistry>>,
        debug: Arc<DebugServices>,
    ) -> Self {
        // Whether backend tools are registered is fixed at construction from
        // whether an engine is present. A reset only ever swaps one engine for
        // another (never Some→None), so this stays correct across resets.
        let has_engine = backend
            .read()
            .expect("backend cell poisoned")
            .engine
            .is_some();
        let tool_router = if has_engine {
            Self::tool_router_ui() + Self::tool_router_backend()
        } else {
            Self::tool_router_ui()
        };
        #[cfg(debug_assertions)]
        let tool_router = tool_router
            + Self::tool_router_reset()
            + Self::tool_router_debug()
            + Self::tool_router_debug_ledgers();

        Self {
            backend,
            type_registry,
            debug,
            watches: Arc::new(Mutex::new(HashMap::new())),
            tool_router,
            dense_projections: crate::dense_projection::ProjectionRegistry::new(
                std::time::Duration::from_secs(60 * 30),
            ),
            session_id: format!("mcp-session:{}", uuid::Uuid::new_v4()),
        }
    }

    /// The live backend engine (cloned from the swappable cell). Panics if not
    /// available — backend tools are only registered when an engine is present,
    /// so this only fires if a backend tool ran in design-gallery mode.
    pub(crate) fn engine(&self) -> Arc<BackendEngine> {
        self.backend
            .read()
            .expect("backend cell poisoned")
            .engine
            .clone()
            .expect(
                "BackendEngine accessed but not available — backend tools should not be \
                 registered in design gallery mode",
            )
    }

    /// The shared service layer over the live engine. Cheap: `HolonService` is
    /// a thin `Arc<BackendEngine>` wrapper, rebuilt per call from the live
    /// cell.
    ///
    /// The facade carries an [`OpOrigin::Agent`] provenance: this server's
    /// stable `session_id` plus a fresh per-call `tool_call_id`, so every
    /// block an agent creates/updates over MCP is stamped for the
    /// supervision view (C2a). One `service()` call maps to one tool
    /// invocation, so the minted id is the revert-whole-call handle.
    pub(crate) fn service(&self) -> HolonService {
        HolonService::new_with_origin(
            self.engine(),
            holon_api::OpOrigin::Agent {
                session_id: self.session_id.clone(),
                tool_call_id: format!("tool-call:{}", uuid::Uuid::new_v4()),
            },
        )
    }

    /// The live builder services (cloned from the swappable cell), if present.
    pub(crate) fn builder_services(&self) -> Option<Arc<dyn BuilderServices>> {
        self.backend
            .read()
            .expect("backend cell poisoned")
            .builder_services
            .clone()
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
        // ALLOW(unused_param): trait signature
        _request: CompleteRequestParam,
        // ALLOW(unused_param): trait signature
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        // Return empty completions - we don't provide argument completions yet
        Ok(CompleteResult {
            completion: CompletionInfo {
                values: vec![],
                has_more: Some(false),
                total: Some(0),
            },
        })
    }
}
