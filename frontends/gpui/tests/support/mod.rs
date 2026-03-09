//! Fast-UI test support — render a `ReactiveViewModel` in `TestAppContext`
//! without any backend, then query `BoundsRegistry` for layout assertions.
//!
//! This is the L2 primitive from the fast UI test plan (see
//! `frontends/gpui/ZED_UI_PATTERNS.md §14`). Production-only concerns
//! (reactive engine, DI, tokio runtime, real session) are replaced with a
//! panicking stub; fixtures that pull on any of those will fail loudly,
//! signalling that the fixture is outside the pure-layout subset.

#![allow(dead_code)] // individual test files use subsets of the helpers

use std::sync::{Arc, OnceLock};

// Each test binary (layout_smoke / layout_proptest / layout_matrix / ...)
// uses a different subset of these re-exports. Allowing unused-imports here
// avoids per-binary warning spam.
#[allow(unused_imports)]
pub use holon_layout_testing::{
    assert_all_nonzero, assert_all_nonzero_except, assert_containment, assert_content_fidelity,
    assert_layout_ok, assert_no_sibling_overlap, assert_nonempty, BlockTreeRegistry,
    BlockTreeThunk, BoundsSnapshot, VISIBLE_LEAF_TYPES,
};

use anyhow::Result;
use gpui::{
    div, point, px, size, AppContext, Bounds, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, Size,
    Styled, TestAppContext, UniformListScrollHandle, Window, WindowBounds, WindowHandle,
    WindowOptions,
};
use holon::entity_profile::RowProfile;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_api::{EntityUri, QueryLanguage};
use holon_frontend::geometry::{ElementInfo, GeometryProvider};
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::RenderContext as FrontendRenderContext;
use holon_frontend::{QueryContext, RowChangeStream, WidgetState};
use holon_gpui::entity_view_registry::{FocusRegistry, LocalEntityScope};
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::render::builders::{self, GpuiRenderContext};

// ── Stub BuilderServices ───────────────────────────────────────────────

/// Every method panics. Fast-UI fixtures must stay in the pure-layout subset
/// (no `BlockRef`, no `EditableText`, no reactive collections). If a fixture
/// pulls on one of these methods, the panic names which capability it touched,
/// so the fixture can be narrowed or moved to the slow E2E layer.
pub struct StubServices;

impl BuilderServices for StubServices {
    fn interpret(&self, _: &RenderExpr, _: &FrontendRenderContext) -> ReactiveViewModel {
        unimplemented!("StubServices::interpret — fixture is not pure-layout")
    }
    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        unimplemented!("StubServices::get_block_data — fixture references a BlockRef")
    }
    fn resolve_profile(&self, _: &DataRow) -> Option<RowProfile> {
        unimplemented!("StubServices::resolve_profile")
    }
    fn compile_to_sql(&self, _: &str, _: QueryLanguage) -> Result<String> {
        unimplemented!("StubServices::compile_to_sql")
    }
    fn start_query(&self, _: String, _: Option<QueryContext>) -> Result<RowChangeStream> {
        unimplemented!("StubServices::start_query")
    }
    fn widget_state(&self, _: &str) -> WidgetState {
        unimplemented!("StubServices::widget_state")
    }
    fn dispatch_intent(&self, _: OperationIntent) {
        unimplemented!("StubServices::dispatch_intent")
    }
    fn runtime_handle(&self) -> tokio::runtime::Handle {
        unimplemented!("StubServices::runtime_handle — fast-UI fixtures are synchronous")
    }
    fn popup_query(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>
    {
        unimplemented!("StubServices::popup_query — fast-UI fixtures don't run popups")
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
/// `runtime_handle` / `popup_query` — it returns real values so that the
/// production `get_or_create_reactive_shell` path works end-to-end inside a
/// fast-UI test.
pub struct TestServices {
    inner: holon_frontend::reactive::StubBuilderServices,
    popup_results: std::sync::Mutex<Vec<DataRow>>,
    registry: Arc<BlockTreeRegistry>,
    /// When true, `runtime_handle()` returns a quiescent current-thread
    /// tokio runtime so driver spawns queue but never execute —
    /// required for fixtures that interpret `Collection`-variant
    /// `ReactiveView`s (scenario proptest) because the default stub
    /// multi-thread runtime trips gpui's `TestScheduler` off-thread
    /// detector. Defaults to `false` so editor / popup tests that
    /// genuinely need async execution keep using the real runtime.
    quiescent_runtime: bool,
}

impl TestServices {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(Vec::new()),
            registry: Arc::new(BlockTreeRegistry::new()),
            quiescent_runtime: false,
        })
    }

    /// Construct with a shared `BlockTreeRegistry` and a quiescent
    /// current-thread runtime handle. Used by `GpuiScenarioSession`,
    /// which needs (a) `watch_live` to resolve block IDs registered by
    /// the session, and (b) reactive-view drivers to queue without
    /// executing (otherwise gpui's `TestScheduler` panics — see
    /// `test_quiescent_runtime_handle`).
    pub fn with_registry_quiescent(registry: Arc<BlockTreeRegistry>) -> Arc<Self> {
        Arc::new(Self {
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(Vec::new()),
            registry,
            quiescent_runtime: true,
        })
    }

    /// Construct a `TestServices` whose `popup_query` returns canned rows.
    /// Used by editor-layer tests that need `LinkProvider` to produce
    /// deterministic items without touching a real SQL backend.
    pub fn with_popup_results(rows: Vec<DataRow>) -> Arc<Self> {
        Arc::new(Self {
            inner: holon_frontend::reactive::StubBuilderServices::new(),
            popup_results: std::sync::Mutex::new(rows),
            registry: Arc::new(BlockTreeRegistry::new()),
            quiescent_runtime: false,
        })
    }
}

impl BuilderServices for TestServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &FrontendRenderContext) -> ReactiveViewModel {
        self.inner.interpret(expr, ctx)
    }
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        self.inner.get_block_data(id)
    }
    fn resolve_profile(&self, row: &DataRow) -> Option<RowProfile> {
        self.inner.resolve_profile(row)
    }
    fn compile_to_sql(&self, query: &str, lang: QueryLanguage) -> Result<String> {
        self.inner.compile_to_sql(query, lang)
    }
    fn start_query(&self, sql: String, ctx: Option<QueryContext>) -> Result<RowChangeStream> {
        self.inner.start_query(sql, ctx)
    }
    fn widget_state(&self, id: &str) -> WidgetState {
        self.inner.widget_state(id)
    }
    fn dispatch_intent(&self, intent: OperationIntent) {
        self.inner.dispatch_intent(intent)
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
    fn popup_query(
        &self,
        _sql: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>
    {
        let rows = self.popup_results.lock().unwrap().clone();
        Box::pin(async move { Ok(rows) })
    }

    fn watch_live(
        &self,
        block_id: &EntityUri,
        _services: Arc<dyn BuilderServices>,
    ) -> holon_frontend::LiveBlock {
        self.registry.watch_live(&block_id.to_string())
    }

    fn set_view_mode(&self, block_id: &EntityUri, mode: String) {
        self.registry.set_active(&block_id.to_string(), &mode);
    }

    fn ui_state(
        &self,
        block_id: &EntityUri,
    ) -> std::collections::HashMap<String, holon_api::Value> {
        let mut out = std::collections::HashMap::new();
        if let Some(name) = self.registry.active_mode_name(&block_id.to_string()) {
            out.insert("view_mode".to_string(), holon_api::Value::String(name));
        }
        out
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
    focus: FocusRegistry,
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
            self.focus.clone(),
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
    focus: FocusRegistry,
    entity_cache: holon_gpui::entity_view_registry::EntityCache,
    fixture_size: Size<Pixels>,
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
            focus: FocusRegistry::new(),
            entity_cache: Default::default(),
            fixture_size,
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
            focus: FocusRegistry::new(),
            entity_cache: Default::default(),
            fixture_size,
        }
    }

    /// Fish out the `ReactiveShell` entity the real render pipeline created
    /// for the (outer) reactive view, keyed by its `stable_cache_key`. Used
    /// by scroll tests to observe the inner `ListState` after wheel events.
    pub fn reactive_shell(
        &self,
        view: &Arc<holon_frontend::reactive_view::ReactiveView>,
    ) -> Option<gpui::Entity<holon_gpui::views::ReactiveShell>> {
        let key = format!("rv-{:016x}", view.stable_cache_key());
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
            self.focus.clone(),
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
        div()
            .size_full()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(builders::render(&self.vm, &gctx))
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
    let services: Arc<dyn BuilderServices> = Arc::new(StubServices);
    let focus = FocusRegistry::new();

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
                    focus,
                })
            },
        )
        .expect("open_window failed")
    });

    cx.run_until_parked();

    let mut entries: Vec<(String, ElementInfo)> = bounds.all_elements();
    // Sort by the `{name}#{seq}` id's numeric suffix so render-tree traversal
    // order is preserved. `all_elements()` returns a HashMap-backed Vec which
    // is unordered otherwise.
    entries.sort_by_key(|(id, _)| seq_from_id(id));

    BoundsSnapshot { entries }
}

/// Reactive counterpart to `render_fixture_sized`. Routes `vm` through the
/// full `ReactiveFixtureView` pipeline (real `builders::render`, real
/// `get_or_create_reactive_shell`, `TestServices` for reactive capabilities),
/// then returns a `BoundsSnapshot` for layout invariants.
///
/// Use this instead of `render_fixture_sized` whenever the tree contains
/// `ReactiveViewKind::Reactive { view }`, `ViewModeSwitcher`, `BlockRef`, or
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

    let mut entries: Vec<(String, ElementInfo)> = bounds.all_elements();
    entries.sort_by_key(|(id, _)| seq_from_id(id));
    BoundsSnapshot { entries }
}

/// Extract the `#{seq}` suffix from an id like `"col#3"`. Unknown ids sort
/// last under `u64::MAX` so a malformed id is visible but not crashing.
fn seq_from_id(id: &str) -> u64 {
    id.rsplit_once('#')
        .and_then(|(_, n)| n.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

// ── GpuiScenarioSession (multi-render window) ─────────────────────────
//
// Keeps a `ReactiveFixtureView` window alive across a whole action sequence
// so the action-aware proptest can render the *same* window multiple times.
// Owns a `BlockTreeRegistry` that is shared with the window's `TestServices`,
// so `apply_action(SwitchViewMode)` pushes a new tree through the registry's
// channel and the ReactiveShell consumer task re-renders.

/// Holds an open `ReactiveFixtureView` window, its bounds registry, and the
/// per-scenario `BlockTreeRegistry`. The registry is shared with the window's
/// `TestServices` so that `apply_action` can switch block modes in-process.
pub struct GpuiScenarioSession {
    bounds: BoundsRegistry,
    registry: Arc<BlockTreeRegistry>,
    _window: WindowHandle<ReactiveFixtureView>,
}

impl GpuiScenarioSession {
    /// Open a window with `vm` as the root, register `blocks` into a fresh
    /// `BlockTreeRegistry`, and run the first render to completion.
    ///
    /// `blocks` is a list of `(block_id, modes, active_mode)` entries — the
    /// same shape as `BlockTreeRegistry::register`. An empty `blocks` list is
    /// fine for scenarios that don't contain any `BlockRef` nodes.
    pub fn open(
        cx: &mut TestAppContext,
        vm: Arc<ReactiveViewModel>,
        blocks: Vec<(String, Vec<(String, BlockTreeThunk)>, usize)>,
        window_size: Size<Pixels>,
    ) -> Self {
        cx.update(|cx| {
            gpui_component::init(cx);
        });

        let registry = Arc::new(BlockTreeRegistry::new());
        for (block_id, modes, active_mode) in blocks {
            registry.register(block_id, modes, active_mode);
        }

        // Shared-registry + quiescent-runtime services: (a) `watch_live`
        // resolves to the session's registered blocks, (b) reactive-view
        // drivers queue onto a current-thread runtime that's never entered,
        // avoiding gpui's `TestScheduler` off-thread panic.
        let services: Arc<dyn BuilderServices> =
            TestServices::with_registry_quiescent(registry.clone()) as Arc<dyn BuilderServices>;
        let bounds = BoundsRegistry::new();
        let bounds_for_view = bounds.clone();

        let window: WindowHandle<ReactiveFixtureView> = cx.update(|cx| {
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
                            services as Arc<dyn BuilderServices>,
                            window_size,
                            bounds_for_view,
                        )
                    })
                },
            )
            .expect("open_window failed")
        });

        let session = Self {
            bounds,
            registry,
            _window: window,
        };
        session.settle(cx);
        session
    }

    /// Apply a `UiInteraction` to the open window and settle the executor.
    ///
    /// For `SwitchViewMode`: locates the canonical VMS button element in the
    /// session's `BoundsRegistry` (id = `vms_button_id_for(block_id, mode)`),
    /// synthesizes a real mouse-down/mouse-up sequence at the button's center
    /// via `VisualTestContext::simulate_event`. The click lands on the
    /// production VMS click handler, which calls `services.set_view_mode`,
    /// which consults *this session's* `BlockTreeRegistry` (because the
    /// window's `TestServices` was built with it) and pushes a fresh tree
    /// down the `LiveBlock` stream. Every pixel of the production path runs.
    ///
    /// **Fails loud** if the target button is missing from `BoundsRegistry`
    /// — a missing button means the scenario produced a `SwitchViewMode`
    /// for a block/mode combination the UI didn't actually render, which
    /// is a scenario-generator bug, not a test runtime to paper over.
    pub fn apply_action(
        &self,
        cx: &mut TestAppContext,
        action: &holon_layout_testing::UiInteraction,
    ) {
        match action {
            holon_layout_testing::UiInteraction::SwitchViewMode {
                block_id,
                target_mode,
            } => {
                let button_id = holon_frontend::vms_button_id_for(block_id, target_mode);
                let info = self.bounds.element_info(&button_id).unwrap_or_else(|| {
                    panic!(
                        "GpuiScenarioSession::apply_action(SwitchViewMode {{ \
                             block_id: {block_id:?}, target_mode: {target_mode:?} }}): \
                             no VMS button bounds recorded under id {button_id:?}. \
                             The scenario generator produced an action for a button \
                             the UI did not render. Either the block is not a VMS, \
                             the target mode is not registered, or the VMS builder \
                             failed to tag the button."
                    )
                });
                let (cx_ref, cy) = info.center();
                let position = gpui::Point::new(px(cx_ref), px(cy));

                let mut vcx = gpui::VisualTestContext::from_window(self._window.into(), cx);
                // Move first so hit-test picks up the non-zero hitbox — same
                // reason `simulate_wheel_at` does this (see its doc).
                vcx.simulate_event(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                });
                vcx.simulate_event(gpui::MouseDownEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    modifiers: gpui::Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                });
                vcx.simulate_event(gpui::MouseUpEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    modifiers: gpui::Modifiers::default(),
                    click_count: 1,
                });
            }
        }
        self.settle(cx);
    }

    /// Drive the gpui executor until no tasks are pending, advance the
    /// fake clock past any pending animation, then drive to quiescence
    /// again. Call after applying an action and before `snapshot()`.
    pub fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    }

    /// Build a `BoundsSnapshot` of the most recently completed render.
    /// Entries are sorted by seq so the snapshot's iteration order
    /// matches render-tree traversal order.
    pub fn snapshot(&self) -> BoundsSnapshot {
        let mut entries: Vec<(String, ElementInfo)> = self.bounds.all_elements();
        entries.sort_by_key(|(id, _)| seq_from_id(id));
        BoundsSnapshot { entries }
    }
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
// - `LayoutMode::BrokenCascade` — replicates the pre-fix cascade pattern
//   (no `size_full`, no `min_h_0`) so tests can prove the broken pattern
//   is actually detectable, not vacuously passing.

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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
// 1. It duplicated production layout code (`ReactiveShell::render`'s
//    list construction, `columns::render`'s non-drawer wrapper) inline
//    in the fixture. Any divergence between fixture and production
//    silently invalidates the test.
//
// 2. The positive control failed: running the same fixture in
//    `Production` mode (wrapped in `scrollable_list_wrapper`) also
//    failed to scroll on wheel events, which meant we couldn't tell
//    whether a failure was a real bug or a fixture artifact.
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
