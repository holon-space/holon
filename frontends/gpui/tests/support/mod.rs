//! Fast-UI test support — render a `ReactiveViewModel` in `TestAppContext`
//! without any backend, then query `BoundsRegistry` for layout assertions.
//!
//! This is the L2 primitive from the fast UI test plan (see
//! `frontends/gpui/ZED_UI_PATTERNS.md §14`). Production-only concerns
//! (reactive engine, DI, tokio runtime, real session) are replaced with a
//! panicking stub; fixtures that pull on any of those will fail loudly,
//! signalling that the fixture is outside the pure-layout subset.

#![allow(dead_code)] // individual test files use subsets of the helpers

use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Result;
use gpui::AppContext;
use gpui::Bounds;
use gpui::Context;
use gpui::ElementId;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Pixels;
use gpui::Point;
use gpui::Render;
use gpui::ScrollDelta;
use gpui::ScrollHandle;
use gpui::ScrollWheelEvent;
use gpui::Size;
use gpui::Styled;
use gpui::TestAppContext;
use gpui::UniformListScrollHandle;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::WindowOptions;
use gpui::div;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::RenderProfile as RowProfile;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::QueryContext;
use holon_frontend::RenderContext as FrontendRenderContext;
use holon_frontend::WidgetState;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::entity_view_registry::LocalEntityScope;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::render::builders::GpuiRenderContext;
use holon_gpui::render::builders::{self};
// Each test binary (layout_smoke / layout_matrix / ...)
// uses a different subset of these re-exports. Allowing unused-imports here
// avoids per-binary warning spam.
#[allow(unused_imports)]
pub use holon_layout_testing::{
    BlockTreeRegistry, BlockTreeThunk, BoundsSnapshot, VISIBLE_LEAF_TYPES, assert_all_nonzero,
    assert_all_nonzero_except, assert_containment, assert_content_fidelity, assert_layout_ok,
    assert_no_sibling_overlap, assert_nonempty,
};

/// Row count each canned `live_query` watcher yields. Chosen so the stacked
/// content is several viewports tall — comfortably taller than any
/// `max_height_fraction` cap a seed would set, so the bounded accordion footer
/// is proven to CLAMP (not merely fit) the greedy live-query shell.
pub const CANNED_LIVE_QUERY_ROWS: usize = 40;

/// A production-faithful `text` row (see `builders/text.rs`): `data.id` plus
/// `props.field == "content"` makes its bounds appear in `BoundsRegistry`
/// keyed by `entity_id` (`{prefix}-{ix}`), so a windowed test can locate the
/// rows a canned `watch_query_live` produced.
fn canned_live_query_row(prefix: &str, ix: usize) -> ReactiveViewModel {
    let id = format!("{prefix}-{ix}");
    let label = format!("{prefix} reference {ix}");
    let mut data = std::collections::HashMap::new();
    data.insert("id".to_string(), holon_api::Value::String(id));
    data.insert(
        "content".to_string(),
        holon_api::Value::String(label.clone()),
    );
    let mut props = std::collections::HashMap::new();
    props.insert("content".to_string(), holon_api::Value::String(label));
    props.insert(
        "field".to_string(),
        holon_api::Value::String("content".to_string()),
    );
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(std::sync::Arc::new(data)).read_only();
    vm
}

// ── Stub BuilderServices ───────────────────────────────────────────────

/// Every method panics. Fast-UI fixtures must stay in the pure-layout subset
/// (no `LiveBlock`, no `EditableText`, no reactive collections). If a fixture
/// pulls on one of these methods, the panic names which capability it touched,
/// so the fixture can be narrowed or moved to the slow E2E layer.
#[derive(Default)]
pub struct StubServices {
    /// Built-in schemes only — a pure-layout fixture has no registry.
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
}

impl BuilderServices for StubServices {
    fn interpret(&self, _: &RenderExpr, _: &FrontendRenderContext) -> ReactiveViewModel {
        unimplemented!("StubServices::interpret — fixture is not pure-layout")
    }
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        unimplemented!("StubServices::clone_arc — fixture reached a lazy widget")
    }
    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
    }
    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        unimplemented!("StubServices::get_block_data — fixture references a LiveBlock")
    }
    fn resolve_profile(&self, _: &DataRow) -> Option<RowProfile> {
        unimplemented!("StubServices::resolve_profile")
    }
    fn watch_query(
        &self,
        _: &str,
        _: QueryLanguage,
        _: Option<QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        unimplemented!("StubServices::watch_query")
    }
    fn widget_state(&self, _: &str) -> WidgetState {
        unimplemented!("StubServices::widget_state")
    }
    fn set_widget_open(&self, _: &str, _: bool) {
        unimplemented!("StubServices::set_widget_open")
    }
    fn dispatch_intent(&self, _: OperationIntent) {
        unimplemented!("StubServices::dispatch_intent")
    }
    fn present_op(
        &self,
        _: holon_api::render_types::OperationDescriptor,
        _: std::collections::HashMap<String, holon_api::Value>,
    ) {
        unimplemented!("StubServices::present_op — fixture is not op-button interactive")
    }
    fn runtime_handle(&self) -> tokio::runtime::Handle {
        unimplemented!("StubServices::runtime_handle — fast-UI fixtures are synchronous")
    }
    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        unimplemented!("StubServices::search_link_candidates — fast-UI fixtures don't run popups")
    }
}

// ── Reactive test services ─────────────────────────────────────────────

/// Full-capability test services for fixtures that render real
/// `ReactiveViewModel::Reactive { view }` trees. Delegates everything to
/// `StubBuilderServices`, which owns a shared process-wide single-threaded
/// tokio runtime and a real shadow interpreter. No `FrontendSession`, no
/// `BackendEngine`, no DI.
///
/// Unlike `StubServices`, `TestServices` does NOT panic on `interpret` /
/// `runtime_handle` / `search_link_candidates` — it returns real values so that
/// the production `get_or_create_reactive_shell` path works end-to-end inside a
/// fast-UI test.
pub struct TestServices {
    inner: holon_frontend::reactive::StubBuilderServices,
    popup_results: std::sync::Mutex<Vec<holon_api::LinkCandidate>>,
    registry: Arc<BlockTreeRegistry>,
    /// Per-drawer open/closed state. Absent = default (open).
    pub drawer_states: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    /// When true, `runtime_handle()` returns a quiescent current-thread
    /// tokio runtime so driver spawns queue but never execute —
    /// required for fixtures that interpret `Collection`-variant
    /// `ReactiveView`s (scenario proptest) because the default stub
    /// multi-thread runtime trips gpui's `TestScheduler` off-thread
    /// detector. Defaults to `false` so editor / popup tests that
    /// genuinely need async execution keep using the real runtime.
    quiescent_runtime: bool,
    /// One-shot initial caret seed recorded by `set_focus_with_caret` — the
    /// same authority `UiState` owns in production, mirrored here so windowed
    /// click tests can assert where a click placed the caret. `None` = no seed
    /// armed (a plain `set_focus`, i.e. caret defaults to end-of-text on
    /// mount).
    caret_seed: std::sync::Mutex<Option<(EntityUri, usize)>>,
    /// Last block handed to `set_focus` / `set_focus_with_caret`.
    focused: std::sync::Mutex<Option<EntityUri>>,
    /// Every intent handed to `dispatch_intent`, in order. Lets a windowed
    /// click test assert WHICH operation a click chose (or that it chose none)
    /// — the stub's own `dispatch_intent` only logs.
    dispatched_intents: std::sync::Mutex<Vec<OperationIntent>>,
    /// The query capability handed to every editor built from this fixture.
    /// `None` installs an ungated [`DeclaresNothingQueryEngine`], so the
    /// vocabulary resolves on the first poll.
    query_engine: Option<Arc<dyn holon_api::QueryEngine>>,
    /// When set, `editable_text` hands this cell to every editor, so the
    /// fixture builds a CELL-ATTACHED (Loro/Full-arm) `EditorView` instead of
    /// the no-cell SqlOnly one. `None` keeps the historical behaviour.
    editable_cell: Option<holon_core::cell::Cell<String>>,
    /// Rows the canned `watch_query_live` yields, when the default
    /// [`CANNED_LIVE_QUERY_ROWS`] is the wrong shape for the fixture. The
    /// shipped left sidebar has ONE integration row, and a section that is
    /// several viewports tall hides defects that only a short section exposes.
    live_query_rows: std::sync::Mutex<Option<usize>>,
    /// Root viewport the production seam reads through `viewport_snapshot`.
    /// `None` reproduces the pre-first-frame state, where the shell has not
    /// pushed a viewport yet.
    viewport: std::sync::Mutex<Option<holon_frontend::AvailableSpace>>,
}

impl TestServices {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            query_engine: None,
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(Vec::new()),
            registry: Arc::new(BlockTreeRegistry::new()),
            drawer_states: std::sync::Mutex::new(std::collections::HashMap::new()),

            quiescent_runtime: false,
            caret_seed: std::sync::Mutex::new(None),
            focused: std::sync::Mutex::new(None),
            dispatched_intents: std::sync::Mutex::new(Vec::new()),
            editable_cell: None,
            live_query_rows: std::sync::Mutex::new(None),
            viewport: std::sync::Mutex::new(None),
        })
    }

    /// A fixture whose editors attach `cell` — the Full/Loro arm, where the
    /// CRDT owns content and the per-row DataRow content subscription is
    /// dropped. Editor code that differs between the arms (anything gated on
    /// `has_cell()`) is only reachable through this constructor.
    /// Build a fixture whose editors resolve their vocabulary through `engine`
    /// — the seam a test uses to hold the resolution window open.
    pub fn with_query_engine(engine: Arc<dyn holon_api::QueryEngine>) -> Arc<Self> {
        let mut this = Self::new();
        Arc::get_mut(&mut this)
            .expect("freshly built, uniquely owned")
            .query_engine = Some(engine);
        this
    }

    pub fn with_editable_cell(cell: holon_core::cell::Cell<String>) -> Arc<Self> {
        Arc::new(Self {
            query_engine: None,
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(Vec::new()),
            registry: Arc::new(BlockTreeRegistry::new()),
            drawer_states: std::sync::Mutex::new(std::collections::HashMap::new()),

            quiescent_runtime: false,
            caret_seed: std::sync::Mutex::new(None),
            focused: std::sync::Mutex::new(None),
            dispatched_intents: std::sync::Mutex::new(Vec::new()),
            editable_cell: Some(cell),
            live_query_rows: std::sync::Mutex::new(None),
            viewport: std::sync::Mutex::new(None),
        })
    }

    /// Every intent this fixture's click handlers dispatched, in order.
    pub fn recorded_intents(&self) -> Vec<OperationIntent> {
        self.dispatched_intents.lock().unwrap().clone()
    }

    /// Construct with a shared `BlockTreeRegistry` and a quiescent
    /// current-thread runtime handle. Used by `GpuiScenarioSession`,
    /// which needs (a) `watch_live` to resolve block IDs registered by
    /// the session, and (b) reactive-view drivers to queue without
    /// executing (otherwise gpui's `TestScheduler` panics — see
    /// `test_quiescent_runtime_handle`).
    pub fn with_registry_quiescent(registry: Arc<BlockTreeRegistry>) -> Arc<Self> {
        Arc::new(Self {
            query_engine: None,
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(Vec::new()),
            registry,
            drawer_states: std::sync::Mutex::new(std::collections::HashMap::new()),

            quiescent_runtime: true,
            caret_seed: std::sync::Mutex::new(None),
            focused: std::sync::Mutex::new(None),
            dispatched_intents: std::sync::Mutex::new(Vec::new()),
            editable_cell: None,
            live_query_rows: std::sync::Mutex::new(None),
            viewport: std::sync::Mutex::new(None),
        })
    }

    /// Construct a `TestServices` whose `search_link_candidates` returns
    /// canned candidates. Used by editor-layer tests that need `LinkProvider`
    /// to produce deterministic items without touching a real SQL backend.
    pub fn with_popup_results(rows: Vec<holon_api::LinkCandidate>) -> Arc<Self> {
        Arc::new(Self {
            query_engine: None,
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(rows),
            registry: Arc::new(BlockTreeRegistry::new()),
            drawer_states: std::sync::Mutex::new(std::collections::HashMap::new()),

            quiescent_runtime: false,
            caret_seed: std::sync::Mutex::new(None),
            focused: std::sync::Mutex::new(None),
            dispatched_intents: std::sync::Mutex::new(Vec::new()),
            editable_cell: None,
            live_query_rows: std::sync::Mutex::new(None),
            viewport: std::sync::Mutex::new(None),
        })
    }

    /// Toggle the open/closed state of a drawer. Returns the new state.
    /// Yield `n` rows from the canned `watch_query_live` instead of
    /// [`CANNED_LIVE_QUERY_ROWS`]. Production's Integrations section has one
    /// row; a 40-row section is taller than the viewport and scrolls
    /// internally, which masks a short scroll extent in the panel above it.
    pub fn set_live_query_rows(&self, n: usize) {
        *self.live_query_rows.lock().unwrap() = Some(n);
    }

    /// Publish a root viewport of `width x height` logical px, as the platform
    /// shell does on launch, resize, and rotation.
    pub fn set_viewport_size(&self, width_px: f32, height_px: f32) {
        *self.viewport.lock().unwrap() = Some(holon_frontend::AvailableSpace {
            width_px,
            height_px,
            width_physical_px: width_px,
            height_physical_px: height_px,
            scale_factor: 1.0,
        });
    }

    pub fn toggle_drawer(&self, block_id: &str) -> bool {
        let mut states = self.drawer_states.lock().unwrap();
        let current = states.get(block_id).copied().unwrap_or(true); // default open
        let new_state = !current;
        states.insert(block_id.to_string(), new_state);
        new_state
    }

    /// The `(block, offset)` caret seed a click armed via
    /// `set_focus_with_caret`, or `None` if the last focus move was a plain
    /// `set_focus` (no caret). Windowed click tests assert on this to prove the
    /// click position plumbed through to caret placement.
    pub fn recorded_caret(&self) -> Option<(EntityUri, usize)> {
        self.caret_seed.lock().unwrap().clone()
    }

    /// The block last focused (with or without a caret seed).
    pub fn focused_block(&self) -> Option<EntityUri> {
        self.focused.lock().unwrap().clone()
    }
}

impl BuilderServices for TestServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &FrontendRenderContext) -> ReactiveViewModel {
        self.inner.interpret(expr, ctx)
    }

    fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        Some(
            self.query_engine
                .clone()
                .unwrap_or_else(|| Arc::new(DeclaresNothingQueryEngine::open())),
        )
    }
    /// Records the intent and answers with no payload — every op the editable
    /// surface dispatches is a plain `set_field`, whose result the adapter does
    /// not read.
    fn dispatch_intent_awaiting_result(
        &self,
        intent: OperationIntent,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<holon_api::Value>>> + Send + 'static>,
    > {
        self.dispatched_intents.lock().unwrap().push(intent);
        Box::pin(std::future::ready(Ok(None)))
    }

    /// Hands out the fixture's cell when one was configured
    /// ([`TestServices::with_editable_cell`]); otherwise the trait default's
    /// error keeps editors in the no-cell SqlOnly arm.
    fn editable_text(
        &self,
        block_id: &EntityUri,
        field: &str,
    ) -> Result<holon_core::cell::Cell<String>> {
        match &self.editable_cell {
            Some(cell) => Ok(cell.clone()),
            None => Err(anyhow::anyhow!(
                "TestServices has no editable cell for {block_id}.{field}"
            )),
        }
    }
    /// Deliberately loud rather than delegating to `inner.clone_arc()`: the
    /// recorded drawer/caret/focus state lives in THIS instance's mutexes, so
    /// a stub handle would interpret deferred content against defaults and the
    /// fixture's assertions would silently read the wrong services.
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        unimplemented!(
            "TestServices::clone_arc — a lazy widget would capture a handle \
             that drops this fixture's recorded state"
        )
    }
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        self.inner.get_block_data(id)
    }
    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        self.inner.link_classifier()
    }
    fn resolve_profile(&self, row: &DataRow) -> Option<RowProfile> {
        self.inner.resolve_profile(row)
    }
    fn watch_query(
        &self,
        _: &str,
        _: QueryLanguage,
        _: Option<QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        // Canned empty stream. `shared_live_query_build` (interpret) only
        // checks Ok-vs-Err then drops the stream, so an immediately-closed
        // channel is enough to let a seeded `live_query(...)` build a real
        // node (query / render_expr props + slot) instead of an error node —
        // the windowed render then reaches `watch_query_live`. The inner
        // `StubBuilderServices::watch_query` bails, which is why we override.
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
    fn widget_state(&self, id: &str) -> WidgetState {
        let states = self.drawer_states.lock().unwrap();
        if let Some(&open) = states.get(id) {
            WidgetState {
                open,
                ..WidgetState::default()
            }
        } else {
            self.inner.widget_state(id)
        }
    }
    fn set_widget_open(&self, id: &str, open: bool) {
        self.drawer_states
            .lock()
            .unwrap()
            .insert(id.to_string(), open);
    }
    fn dispatch_intent(&self, intent: OperationIntent) {
        self.dispatched_intents.lock().unwrap().push(intent.clone());
        self.inner.dispatch_intent(intent)
    }
    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        ctx_params: std::collections::HashMap<String, holon_api::Value>,
    ) {
        // Delegate to the inner stub, which panics. Tests that exercise ops
        // must use the production click path (ClickBlock + popup
        // interaction), not a shortcut via op_button in headless mode.
        self.inner.present_op(op, ctx_params)
    }
    fn runtime_handle(&self) -> tokio::runtime::Handle {
        if self.quiescent_runtime {
            // `StubBuilderServices` normally builds a multi-thread
            // tokio runtime named `stub-builder-services`. Any task
            // spawned on that runtime runs on its worker thread,
            // which gpui's `TestScheduler` detects as off-thread
            // activity and panics with "Your test is not
            // deterministic." — see
            // `zed/crates/scheduler/src/test_scheduler.rs:111`. The
            // scenario proptest opts into this quiescent handle so
            // `start_reactive_views` can call `view.start()` and
            // spawn drivers without ever executing them — our
            // fixture pre-populates items synchronously, so the
            // drivers would be a duplicate of work already done.
            test_quiescent_runtime_handle()
        } else {
            self.inner.runtime_handle()
        }
    }
    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        let rows = self.popup_results.lock().unwrap().clone();
        Box::pin(async move { Ok(rows) })
    }

    fn watch_live(
        &self,
        block_id: &EntityUri,
        _: Arc<dyn BuilderServices>,
    ) -> holon_frontend::LiveBlock {
        self.registry.watch_live(&block_id.to_string())
    }

    /// Feed a seeded `live_query(...)` with canned rows so it renders a REAL
    /// greedy `ReactiveShell` (plan R4: `flex_grow:1` + height `relative(1.0)`)
    /// instead of panicking through the default trait impl. The row set is
    /// deliberately taller than any sane accordion cap, so a windowed smoke can
    /// prove the bounded footer CLAMPS the greedy shell rather than the shell
    /// happening to be short. Rows are a plain `column` of data-bound `text`
    /// VMs (NO `ReactiveView` collection), so `start_reactive_views` spawns no
    /// driver — nothing runs on a tokio worker for gpui's `TestScheduler` to
    /// flag. Backlinks vs outline are told apart by the query text, so this one
    /// impl feeds both of the seeded main panel's `live_query`s.
    fn watch_query_live(
        &self,
        query: String,
        _: QueryLanguage,
        _: RenderExpr,
        _: Option<QueryContext>,
        _: Arc<dyn BuilderServices>,
    ) -> (EntityUri, holon_frontend::LiveBlock) {
        let (prefix, key) = if query.contains("backlinks") {
            ("backlink", "query:canned-backlinks")
        } else if query.contains("integration_state") {
            ("integration", "query:canned-integrations")
        } else {
            ("outline", "query:canned-outline")
        };
        // A static `list` COLLECTION — the shape production's streaming
        // `watch_query_live` hands back (query rows woven through the
        // item_template). Rendering it through the real gpui list path gives the
        // greedy `live_query` `ReactiveShell` a content-height inner, so it
        // reports real height (and the bounded accordion body caps it). A plain
        // `column` would collapse the `relative(1.0)` shell to 0.
        let row_count = self
            .live_query_rows
            .lock()
            .unwrap()
            .unwrap_or(CANNED_LIVE_QUERY_ROWS);
        let rows: Vec<ReactiveViewModel> = (0..row_count)
            .map(|i| canned_live_query_row(prefix, i))
            .collect();
        let view = std::sync::Arc::new(
            holon_frontend::reactive_view::ReactiveView::new_static_with_layout(
                rows,
                holon_frontend::reactive_view_model::CollectionVariant::list(0.0),
            ),
        );
        let tree = ReactiveViewModel {
            collection: Some(view),
            ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
        };
        let live_block = holon_frontend::LiveBlock {
            tree,
            structural_changes: Box::pin(futures::stream::pending()),
            watch_guard: None,
        };
        // ALLOW(entity_uri_from_raw): synthetic canned-watcher key, no upstream URI
        (EntityUri::from_raw(key), live_block)
    }

    fn ui_state(
        &self,
        block_id: &EntityUri,
    ) -> std::collections::HashMap<String, holon_api::Value> {
        let mut out = std::collections::HashMap::new();
        let key = block_id.to_string();
        if let Some(name) = self.registry.active_mode_name(&key) {
            out.insert("view_mode".to_string(), holon_api::Value::String(name));
        }
        out
    }

    fn viewport_snapshot(&self) -> Option<holon_frontend::AvailableSpace> {
        *self.viewport.lock().unwrap()
    }

    // ── Focus + caret seed (mirrors `UiState`) ─────────────────────────────
    // The default trait bodies drop the caret offset and return `None`; these
    // overrides record it so windowed click tests can assert caret placement.

    fn set_focus(&self, block: Option<EntityUri>) {
        // Age out a stale seed unless it targets the block we're focusing —
        // matches `UiState::set_focus` so a plain click on a block defaults
        // that block's caret to end-of-text.
        {
            let mut seed = self.caret_seed.lock().unwrap();
            if let Some((ref seed_block, _)) = *seed {
                if Some(seed_block) != block.as_ref() {
                    *seed = None;
                }
            }
        }
        *self.focused.lock().unwrap() = block;
    }

    fn set_focus_with_caret(&self, block: EntityUri, offset: usize) {
        *self.caret_seed.lock().unwrap() = Some((block.clone(), offset));
        *self.focused.lock().unwrap() = Some(block);
    }

    fn peek_caret_seed(&self, block: &EntityUri) -> Option<usize> {
        match &*self.caret_seed.lock().unwrap() {
            Some((seed_block, offset)) if seed_block == block => Some(*offset),
            _ => None,
        }
    }

    fn consume_caret_seed(&self, block: &EntityUri) {
        let mut seed = self.caret_seed.lock().unwrap();
        if let Some((ref seed_block, _)) = *seed {
            if seed_block == block {
                *seed = None;
            }
        }
    }
}

/// A process-global `current_thread` tokio runtime used solely for its
/// `Handle`. Handed to `BuilderServices::runtime_handle()` so any task
/// spawned by fixture-driven reactive code (drivers, signal pipelines)
/// queues into this runtime instead of the stub multi-thread one. No
/// call-site ever runs `rt.block_on(...)` on it, so the queued tasks
/// never execute — they sit there until the process exits. That's
/// intentional: the fixture pre-populates everything it needs
/// synchronously, so the drivers would be a duplicate of work already
/// done, and letting them run is what triggers gpui's off-thread panic.
fn test_quiescent_runtime_handle() -> tokio::runtime::Handle {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build quiescent tokio runtime for fixture")
    })
    .handle()
    .clone()
}

// ── FixtureView ────────────────────────────────────────────────────────

/// A minimal `gpui::Render` view that holds a `ReactiveViewModel` and renders
/// it through `render::builders::render`. Mirrors the real view wiring but
/// without session or tokio runtime.
pub struct FixtureView {
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
    bounds: BoundsRegistry,
}

impl FixtureView {
    /// Host `vm` in a window with caller-supplied `services` (e.g. a
    /// `TestServices` whose caret seed the test asserts on) and a shared
    /// `bounds` registry the caller snapshots after `run_until_parked`.
    pub fn new(
        vm: Arc<ReactiveViewModel>,
        services: Arc<dyn BuilderServices>,
        bounds: BoundsRegistry,
    ) -> Self {
        Self {
            vm,
            services,
            bounds,
        }
    }
}

impl Render for FixtureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Matches HolonApp::render: reset the registry at the top of every
        // render pass so bounds from the previous pass don't pollute queries.
        self.bounds.begin_pass();

        let ctx = FrontendRenderContext::default();
        let gctx = GpuiRenderContext::new(
            ctx,
            self.services.clone(),
            self.bounds.clone(),
            LocalEntityScope::new(),
            NavigationState::new(),
            window,
            cx,
        );
        builders::render(&self.vm, &gctx)
    }
}

// ── Reactive fixture (persistent, with entity cache) ─────────────────

/// A persistent `Render` view that routes a real `ReactiveViewModel::Reactive`
/// tree through `render::builders::render`. Unlike `FixtureView`, this owns a
/// stable `EntityCache` so the inner `ReactiveShell` entity (created lazily
/// by `get_or_create_reactive_shell`) survives across re-renders — and
/// survives long enough for hitboxes to register and wheel events to route.
///
/// Uses `TestServices` so the production `interpret` / `runtime_handle` paths
/// are real. The outer `size_full()` wrapper plus an explicit
/// `fixture_size` gives the inner reactive shell a definite-sized parent
/// (otherwise flex chains inside `columns::render` resolve to zero).
pub struct ReactiveFixtureView {
    vm: Arc<ReactiveViewModel>,
    services: Arc<dyn BuilderServices>,
    bounds: BoundsRegistry,
    entity_cache: holon_gpui::entity_view_registry::EntityCache,
    fixture_size: Size<Pixels>,
    /// Height of the window chrome production stacks ABOVE the content
    /// wrapper (title bar, tab strip, breadcrumb bar). `0.0` mounts the
    /// content wrapper as the whole window — the historical fixture shape.
    /// See [`Self::with_page_chrome`].
    chrome_height: f32,
}

impl ReactiveFixtureView {
    pub fn new(vm: Arc<ReactiveViewModel>, fixture_size: Size<Pixels>) -> Self {
        Self::with_bounds(vm, fixture_size, BoundsRegistry::new())
    }

    /// Construct with an externally-owned `BoundsRegistry` so the caller can
    /// snapshot element bounds after rendering completes. Cloning the
    /// registry shares state (it's Arc-backed), so the external handle sees
    /// every widget the render pass records.
    pub fn with_bounds(
        vm: Arc<ReactiveViewModel>,
        fixture_size: Size<Pixels>,
        bounds: BoundsRegistry,
    ) -> Self {
        Self::with_bounds_and_services(
            vm,
            fixture_size,
            bounds,
            TestServices::new() as Arc<dyn BuilderServices>,
        )
    }

    /// Construct with an externally-supplied `BuilderServices`. Used by
    /// `ScenarioSession`, which wants a quiescent-runtime `TestServices`
    /// so drivers spawned by `start_reactive_views` queue but don't run.
    pub fn with_bounds_and_services(
        vm: Arc<ReactiveViewModel>,
        fixture_size: Size<Pixels>,
        bounds: BoundsRegistry,
        services: Arc<dyn BuilderServices>,
    ) -> Self {
        Self {
            vm,
            services,
            bounds,
            entity_cache: Default::default(),
            fixture_size,
            chrome_height: 0.0,
        }
    }

    /// Construct with both an externally-owned `BoundsRegistry` and a
    /// pre-built `Arc<dyn BuilderServices>`. Used by `GpuiScenarioSession`
    /// to inject a `TestServices` that shares the same `BlockTreeRegistry`
    /// as the session's block registrations.
    pub fn with_services_and_bounds(
        vm: Arc<ReactiveViewModel>,
        services: Arc<dyn BuilderServices>,
        fixture_size: Size<Pixels>,
        bounds: BoundsRegistry,
    ) -> Self {
        Self {
            vm,
            services,
            bounds,
            entity_cache: Default::default(),
            fixture_size,
            chrome_height: 0.0,
        }
    }

    /// Stack `height` px of window chrome ABOVE the content wrapper, inside
    /// production's `page` div (`lib.rs` `HolonApp::render`: `size_full
    /// flex_col`, children `title_bar` → optional tab strip → optional
    /// breadcrumb bar → content). Without this the fixture mounts the content
    /// wrapper as the whole window, so any geometry defect that only appears
    /// when the wrapper has a preceding sibling is invisible to it.
    pub fn with_page_chrome(mut self, height: f32) -> Self {
        self.chrome_height = height;
        self
    }

    /// Fish out the `ReactiveShell` entity the real render pipeline created
    /// for the (outer) reactive view, keyed by its `stable_cache_key`. Used
    /// by scroll tests to observe the inner `ListState` after wheel events.
    pub fn reactive_shell(
        &self,
        view: &Arc<holon_frontend::reactive_view::ReactiveView>,
    ) -> Option<gpui::Entity<holon_gpui::views::ReactiveShell>> {
        let key =
            holon_gpui::entity_view_registry::CacheKey::ReactiveShell(view.stable_cache_key());
        let cache = self.entity_cache.read().unwrap();
        cache.get(&key).and_then(|any| any.clone().downcast().ok())
    }
}

impl Render for ReactiveFixtureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bounds.begin_pass();
        let ctx = FrontendRenderContext::default();
        let local = LocalEntityScope::new().with_cache(self.entity_cache.clone());
        let gctx = GpuiRenderContext::new(
            ctx,
            self.services.clone(),
            self.bounds.clone(),
            local,
            NavigationState::new(),
            window,
            cx,
        );
        // Wrap in a `size_full flex_1 flex flex_col overflow_hidden`
        // div to mirror the HolonApp root content wrapper exactly
        // (`frontends/gpui/src/lib.rs` `HolonApp::render`). Any deviation
        // leaks into layout assertions: production nests every reactive
        // collection inside this specific flex-col overflow-hidden chain,
        // so percentage sizing that resolves differently here vs. a
        // plain `size_full` root would silently mask real regressions.
        let content = div()
            .size_full()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(builders::render(&self.vm, &gctx));
        if self.chrome_height <= 0.0 {
            return content.into_any_element();
        }
        // Production's `page` (`lib.rs` `HolonApp::render`): `size_full
        // flex_col` stacking the title bar (plus optional tab strip and
        // breadcrumb bar) ABOVE the content wrapper.
        // Production's OWN page container (`lib.rs`), not a copy of it: the
        // layout defect this fixture exists to catch lives in that chain, so
        // modelling it here would only ever assert the model.
        holon_gpui::page_container()
            .child(div().w_full().flex_shrink_0().h(px(self.chrome_height)))
            .child(content)
            .into_any_element()
    }
}

// ── Rendering primitive ────────────────────────────────────────────────
// `BoundsSnapshot` is re-exported from `holon_layout_testing` at the top.

/// Default fixture window size — matches the "desktop" case. Use
/// `render_fixture_sized` when you need to sweep layout across sizes.
pub fn default_fixture_size() -> Size<Pixels> {
    size(px(800.0), px(600.0))
}

/// Dispatch a scroll wheel at `position` with vertical pixel delta.
///
/// Sends a `MouseMoveEvent` first so gpui's `mouse_hit_test` picks up any
/// hitbox not at `(0, 0)`. Without the move, the initial hit-test stays at
/// the window origin and the wheel event routes to nothing — tests pass
/// structurally but the assertion fails with "offset didn't change" and
/// the root cause is invisible. Always use this helper for wheel tests.
pub fn simulate_wheel_at(
    vcx: &mut gpui::VisualTestContext,
    position: Point<Pixels>,
    delta_y: Pixels,
) {
    vcx.simulate_event(gpui::MouseMoveEvent {
        position,
        ..Default::default()
    });
    vcx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.0), delta_y)),
        ..Default::default()
    });
}

/// Render a fixture `ReactiveViewModel` inside `TestAppContext` at the default
/// window size and return a `BoundsSnapshot` with every widget's final bounds.
///
/// Initializes `gpui_component` theme on first call (idempotent) and opens an
/// off-screen test window. Returns after `run_until_parked()` so all layout
/// and prepaint work has completed.
pub fn render_fixture(cx: &mut TestAppContext, vm: Arc<ReactiveViewModel>) -> BoundsSnapshot {
    render_fixture_sized(cx, vm, default_fixture_size())
}

/// Render a fixture at an explicit window size. Use this to sweep a fixture
/// across (narrow, default, wide) sizes to catch min-width / min-height
/// regressions that only show up at certain viewports.
///
/// Drops into `cx.update(|app| app.open_window(...))` directly rather than
/// using a higher-level `TestAppContext::open_window` helper because that
/// helper was added to gpui after our pinned zed rev. Behaviorally equivalent.
pub fn render_fixture_sized(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    window_size: Size<Pixels>,
) -> BoundsSnapshot {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let bounds = BoundsRegistry::new();
    let services: Arc<dyn BuilderServices> = Arc::new(StubServices::default());

    let bounds_for_view = bounds.clone();
    let _window: WindowHandle<FixtureView> = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| FixtureView {
                    vm,
                    services,
                    bounds: bounds_for_view,
                })
            },
        )
        .expect("open_window failed")
    });

    cx.run_until_parked();

    holon_layout_testing::snapshot::snapshot_from_provider(&bounds)
}

/// Reactive counterpart to `render_fixture_sized`. Routes `vm` through the
/// full `ReactiveFixtureView` pipeline (real `builders::render`, real
/// `get_or_create_reactive_shell`, `TestServices` for reactive capabilities),
/// then returns a `BoundsSnapshot` for layout invariants.
///
/// Use this instead of `render_fixture_sized` whenever the tree contains
/// collection-backed nodes, `ViewModeSwitcher`, `LiveBlock`, or
/// any other variant that `StubServices` would panic on.
pub fn render_reactive_fixture_sized(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    window_size: Size<Pixels>,
) -> BoundsSnapshot {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();

    let _window: WindowHandle<ReactiveFixtureView> = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| ReactiveFixtureView::with_bounds(vm, window_size, bounds_for_view))
            },
        )
        .expect("open_window failed")
    });

    cx.run_until_parked();
    // Advance the simulated clock past any ongoing drawer / view-mode-switcher
    // animation so layout invariants see the settled end state, not the first
    // animation frame. 500ms is well over the current `SLIDE_DURATION`
    // (150ms) and cheap in a fake-time executor.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();

    holon_layout_testing::snapshot::snapshot_from_provider(&bounds)
}

/// Like `render_reactive_fixture_sized`, but drives the tree through a
/// QUIESCENT-runtime `TestServices` (current-thread handle: driver / signal
/// spawns queue but never execute). Required whenever the tree interprets or
/// renders `text(col(...))` or a `live_query(...)` shell — both spawn tokio
/// subscriptions that, on the default multi-thread stub runtime, trip gpui's
/// `TestScheduler` off-thread detector. Also the entry point that reaches this
/// module's canned `watch_query` / `watch_query_live`.
pub fn render_reactive_fixture_quiescent_sized(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    window_size: Size<Pixels>,
) -> BoundsSnapshot {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let services: Arc<dyn BuilderServices> =
        TestServices::with_registry_quiescent(Arc::new(BlockTreeRegistry::new()));

    let _window: WindowHandle<ReactiveFixtureView> = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| {
                    ReactiveFixtureView::with_services_and_bounds(
                        vm,
                        services,
                        window_size,
                        bounds_for_view,
                    )
                })
            },
        )
        .expect("open_window failed")
    });

    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();

    holon_layout_testing::snapshot::snapshot_from_provider(&bounds)
}

/// Like `render_reactive_fixture_quiescent_sized` but with CALLER-supplied
/// services — used to route through a real `live_block(block:default-*)` shell
/// by handing in a `TestServices` whose `BlockTreeRegistry` has the block
/// registered (production-faithful composition).
pub fn render_reactive_fixture_quiescent_sized_with_services(
    cx: &mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    window_size: Size<Pixels>,
    services: Arc<dyn BuilderServices>,
) -> BoundsSnapshot {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();

    let _window: WindowHandle<ReactiveFixtureView> = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| {
                    ReactiveFixtureView::with_services_and_bounds(
                        vm,
                        services,
                        window_size,
                        bounds_for_view,
                    )
                })
            },
        )
        .expect("open_window failed")
    });

    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();

    holon_layout_testing::snapshot::snapshot_from_provider(&bounds)
}

// ── Scroll fixture ────────────────────────────────────────────────────
//
// A `gpui::Render` view that hosts a `uniform_list` inside the **same**
// production layout chain that wraps `ReactiveShell` in `builders::render`
// (via `scrollable_list_wrapper`). Exposes the `UniformListScrollHandle`
// so tests can observe scroll state before/after simulating input.
//
// Two layout modes:
// - `LayoutMode::Production` — calls `scrollable_list_wrapper`. Should work.
// - `LayoutMode::BrokenCascade` — replicates the pre-fix cascade pattern (no
//   `size_full`, no `min_h_0`) so tests can prove the broken pattern is
//   actually detectable, not vacuously passing.

#[derive(Clone, Copy, Debug)]
pub enum LayoutMode {
    /// Production chain: `scrollable_list_wrapper` with `size_full`,
    /// `flex_1 + min_h_0 + w_full` on the inner div. Scroll works.
    Production,
    /// Pre-fix chain: the inner div uses only `flex_1` without `min_h_0`
    /// or `w_full`, and the outer lacks `size_full`. Taffy uses the list's
    /// content height as the viewport minimum; `scroll_max = 0`; the list
    /// silently fails to scroll. This is the April 2026 cascade bug.
    ///
    /// NOTE: currently an unreliable negative control because the
    /// `uniform_list` in `ScrollableListView` has `.h_full()` which
    /// bypasses the flex chain — a direct h_full on the list resolves
    /// regardless of whether the wrapper propagates height correctly.
    /// Kept for layout-mode-parity only; a real negative control needs
    /// the Option B real-render path.
    BrokenCascade,
}

pub struct ScrollableListView {
    item_count: usize,
    item_height: Pixels,
    scroll_handle: UniformListScrollHandle,
    mode: LayoutMode,
    bounds: BoundsRegistry,
    /// Explicit pixel dimensions applied to the view's outermost div.
    /// Required in tests because `cx.draw` with `AvailableSpace::Definite`
    /// isn't sufficient to let `size_full()` on a descendant resolve to a
    /// concrete pixel height at the pinned gpui rev — we need an
    /// explicitly-sized parent for the chain to propagate.
    fixture_size: Size<Pixels>,
}

impl ScrollableListView {
    pub fn new(item_count: usize, mode: LayoutMode, bounds: BoundsRegistry) -> Self {
        Self::with_handle(item_count, mode, bounds, UniformListScrollHandle::new())
    }

    /// Build a scrollable view whose `UniformListScrollHandle` is owned by
    /// the caller (so the caller can observe scroll state after wheel
    /// events). This is the constructor interaction tests use.
    pub fn with_handle(
        item_count: usize,
        mode: LayoutMode,
        bounds: BoundsRegistry,
        scroll_handle: UniformListScrollHandle,
    ) -> Self {
        Self {
            item_count,
            item_height: px(24.0),
            scroll_handle,
            mode,
            bounds,
            fixture_size: size(px(600.0), px(400.0)),
        }
    }

    pub fn with_fixture_size(mut self, s: Size<Pixels>) -> Self {
        self.fixture_size = s;
        self
    }

    pub fn scroll_handle(&self) -> UniformListScrollHandle {
        self.scroll_handle.clone()
    }

    /// The inner `ScrollHandle` used by `uniform_list` under the hood.
    /// Tests use this to read **applied** scroll state — `offset()`,
    /// `max_offset()`, `logical_scroll_top()` — rather than
    /// `UniformListScrollHandle::logical_scroll_top_index()`, which
    /// returns the *pending* deferred scroll target and is useless for
    /// verifying that scrolling actually happened.
    pub fn base_scroll_handle(&self) -> ScrollHandle {
        self.scroll_handle.0.borrow().base_handle.clone()
    }

    /// Total intrinsic content height (item_count × item_height). Useful
    /// for assertions that require knowing whether the list should overflow
    /// its viewport without poking at gpui internals.
    pub fn total_content_height(&self) -> Pixels {
        self.item_height * self.item_count as f32
    }
}

impl Render for ScrollableListView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.bounds.begin_pass();

        let item_count = self.item_count;
        let item_height = self.item_height;
        let scroll_handle = self.scroll_handle.clone();

        // Build the uniform_list element. Uses `.w_full().flex_grow()` —
        // the exact pattern production's `ReactiveShell::render` applies
        // to `gpui::list(...).with_sizing_behavior(Infer)`. We use
        // `uniform_list` here because it requires less scaffolding than
        // variable-height `list`, but the sizing chain is identical —
        // both rely on the flex ancestor chain to give them a definite
        // height via `flex_grow`.
        //
        // This is DELIBERATELY different from the simpler `.h_full()`
        // pattern the gpui list.rs tests use. `h_full` would mask any
        // flex-chain regression because it resolves against whatever
        // the parent's height is (definite or not). `flex_grow` only
        // works when the flex ancestor chain actually propagates
        // definite height down through `min_h_0` / `overflow_hidden`.
        let list = gpui::uniform_list(
            "scroll-fixture-list",
            item_count,
            move |range: std::ops::Range<usize>, _window, _cx| {
                range
                    .map(|i| {
                        div()
                            .h(item_height)
                            .w_full()
                            .child(format!("item {i}"))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&scroll_handle)
        .w_full()
        .h_full();

        // Return the wrapper directly from the view's render with NO
        // intermediate div. `cx.draw` gives the view's root element its
        // available space, so the wrapper's `size_full()` resolves
        // against that directly.
        match self.mode {
            LayoutMode::Production => holon_gpui::render::builders::scrollable_list_wrapper(
                list,
                ElementId::from("scroll-fixture"),
            ),
            LayoutMode::BrokenCascade => {
                // Pre-fix cascade: `flex_1` only, no `min_h_0`, no
                // `overflow_hidden`. Because the inner only has `flex_1`
                // with no `min_h_0`, Taffy takes the content height as
                // the minimum and the inner inflates to content size,
                // making the list viewport equal its content.
                div()
                    .id(ElementId::from("scroll-fixture-broken"))
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().child(list))
                    .into_any_element()
            }
        }
    }
}

// ── Removed: `ScrollableGpuiListView` and `LayoutMode::ColumnsChild` ──
//
// A hand-rolled `gpui::list` fixture + inline copy of
// `columns::render`'s panel wrapper chain lived here briefly while
// reproducing "scrolling inside columns doesn't work for tree views".
// Removed because the approach was fundamentally unreliable:
//
// 1. It duplicated production layout code (`ReactiveShell::render`'s list
//    construction, `columns::render`'s non-drawer wrapper) inline in the
//    fixture. Any divergence between fixture and production silently
//    invalidates the test.
//
// 2. The positive control failed: running the same fixture in `Production` mode
//    (wrapped in `scrollable_list_wrapper`) also failed to scroll on wheel
//    events, which meant we couldn't tell whether a failure was a real bug or a
//    fixture artifact.
//
// The reproduction is being moved to the **real render path** in a
// separate session ("Option B") — implementing enough of
// `BuilderServices` to render actual `ReactiveViewModel::Reactive`
// trees with canned data, so fast-layer tests exercise production
// code verbatim.
//
// See the doc block at the top of `tests/layout_scroll.rs` for the
// current hypothesis about the columns bug and the recommended fix
// site (`builders/columns.rs` around line 118).

/// A `QueryEngine` that answers only the two reads the editable surface's
/// SOURCE PROJECTION needs, and answers them honestly: this fixture's document
/// declares no `#+TODO:` line, so `block_todo_keywords` returns `None` and the
/// parser's defaults ARE its vocabulary.
///
/// Without a query capability at all, an editor cannot RESOLVE a vocabulary and
/// therefore does not classify its surface (`Surface::Pending`) — correct in
/// production, but it would make every projection rung here vacuous.
pub struct DeclaresNothingQueryEngine {
    /// When present, `block_todo_keywords` AWAITS this before answering — the
    /// vocabulary-resolution window, held open under the test's control instead
    /// of raced. `None` answers immediately.
    gate: std::sync::Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
}

impl DeclaresNothingQueryEngine {
    /// Answers immediately: the window is closed before anything can observe
    /// it.
    pub fn open() -> Self {
        Self {
            gate: std::sync::Mutex::new(None),
        }
    }

    /// Holds the vocabulary read open until the returned sender fires, so a
    /// test can assert what the editor does WHILE the window is open and
    /// then again after it closes.
    pub fn gated() -> (Self, futures::channel::oneshot::Sender<()>) {
        let (tx, rx) = futures::channel::oneshot::channel();
        (
            Self {
                gate: std::sync::Mutex::new(Some(rx)),
            },
            tx,
        )
    }
}

#[async_trait::async_trait]
impl holon_api::QueryEngine for DeclaresNothingQueryEngine {
    async fn lookup_block_path(&self, _: &holon_api::EntityUri) -> Result<String> {
        Ok(String::new())
    }

    async fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: std::collections::HashMap<String, holon_api::Value>,
        _: Option<holon_api::QueryContext>,
        _: holon_api::render_requirements::RenderRequirements,
    ) -> Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("DeclaresNothingQueryEngine does not watch queries")
    }

    async fn search_link_candidates(&self, _: &str) -> Result<Vec<holon_api::LinkCandidate>> {
        Ok(Vec::new())
    }

    async fn block_content_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        Ok(None)
    }

    async fn block_task_state_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        Ok(None)
    }

    async fn block_todo_keywords(
        &self,
        _: &holon_api::EntityUri,
    ) -> Result<Option<Vec<holon_api::TaskState>>> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.await;
        }
        Ok(None)
    }
}
