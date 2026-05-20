//! GPUI GeometryProvider — reads from a shared BoundsRegistry populated during render.
//!
//! `BoundsTracker` is a transparent wrapper element that records the computed bounds
//! of its child into the `BoundsRegistry` during the prepaint phase. Use `tracked()`
//! to wrap any element that should be locatable for click-based PBT testing.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use holon_frontend::geometry::{ElementInfo, GeometryProvider};
use holon_frontend::size_expectation::SizeBounds;

/// Shared registry of element metadata, populated during GPUI render passes.
///
/// Double-buffered: writes during a render pass go to `staged`, reads come from
/// `committed`. At the start of each render pass, `begin_pass()` atomically moves
/// the previous `staged` into `committed` and resets `staged` for the new pass.
///
/// This gives readers a consistent snapshot of the PREVIOUS fully-completed render:
///   - Frame N writes populate `staged`.
///   - Frame N+1's `begin_pass()` moves frame N's data into `committed`.
///   - Readers see frame N's data until frame N+2 arrives.
///
/// Note: readers see data that is one frame behind. This is fine because GPUI
/// renders continuously and tests wait (sleep + settle) between state mutations,
/// so `committed` reflects the stable "last complete render" by the time tests read.
/// If the UI becomes empty in a re-render, that propagates to `committed` after the
/// next pass, so empty-UI regressions are detected.
#[derive(Clone)]
pub struct BoundsRegistry {
    inner: Arc<RwLock<BoundsState>>,
    /// Woken on every committed-buffer rotation (`begin_pass`/`flush`) and on
    /// cold-phase `record`s — i.e. whenever `committed` may have changed.
    /// Backs [`GeometryProvider::changed`] so test wait-loops wake per frame
    /// commit instead of sleeping a fixed interval.
    commit_notify: Arc<tokio::sync::Notify>,
}

struct BoundsState {
    staged: HashMap<String, ElementInfo>,
    committed: HashMap<String, ElementInfo>,
    /// Monotonic counter for auto-assigned element ids within a render pass.
    /// Reset to 0 at the start of each `begin_pass()`. Used by `tag()` in
    /// `render::builders` so every widget in a pass gets a unique key like
    /// `"col#3"`, letting tests enumerate the tree in render order.
    seq: u64,
    /// True until the first `begin_pass()` call successfully rotates a
    /// non-empty staged buffer into committed. While cold, every `record()`
    /// also writes to committed so single-frame readers (fast UI tests) see
    /// the full tree instead of just the first-recorded widget.
    cold: bool,
    /// Monotonic count of committed-buffer rotations (each non-empty
    /// `begin_pass`/`flush`). Lets a reader detect that a *fresh* frame has
    /// painted even when both the old and new frames are non-empty — needed by
    /// the windowed capture-minimizer, which re-points one window at successive
    /// SUTs and must wait for the rebound frame, not read the previous one.
    committed_gen: u64,
}

impl Default for BoundsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BoundsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BoundsState {
                staged: HashMap::new(),
                committed: HashMap::new(),
                seq: 0,
                cold: true,
                committed_gen: 0,
            })),
            commit_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Record element metadata during prepaint. Writes go to the staged buffer.
    ///
    /// While the registry is still "cold" (no `begin_pass()` has yet rotated
    /// a non-empty staged buffer into committed), every record is also written
    /// to committed so one-shot readers — fast UI tests, the very first render
    /// of a fresh app — see the full tree instead of just the first-recorded
    /// widget.
    ///
    /// Once a real pass has completed, cold start is over and subsequent
    /// records hit only staged (standard double-buffering).
    pub fn record(&self, id: String, info: ElementInfo) {
        let mut state = self.inner.write().unwrap();
        state.staged.insert(id.clone(), info.clone());
        if state.cold {
            state.committed.insert(id, info);
            drop(state);
            self.commit_notify.notify_waiters();
        }
    }

    /// Allocate a fresh per-pass sequence number. Used by `tag()` to mint
    /// unique element ids within a render pass.
    pub fn next_seq(&self) -> u64 {
        let mut state = self.inner.write().unwrap();
        let s = state.seq;
        state.seq += 1;
        s
    }

    /// Begin a new render pass. Promotes staged → committed (if staged has data),
    /// resets staged for the new pass, and resets the per-pass sequence counter.
    ///
    /// The first successful rotation (non-empty staged) clears the cold-start
    /// flag — from then on records only hit staged.
    pub fn begin_pass(&self) {
        let mut state = self.inner.write().unwrap();
        let new = std::mem::take(&mut state.staged);
        let rotated = !new.is_empty();
        if rotated {
            state.committed = new;
            state.cold = false;
            state.committed_gen += 1;
        }
        state.seq = 0;
        drop(state);
        if rotated {
            self.commit_notify.notify_waiters();
        }
    }

    /// Number of committed-buffer rotations so far (see [`BoundsState::committed_gen`]).
    /// Strictly increases on each non-empty `begin_pass`/`flush`.
    pub fn committed_generation(&self) -> u64 {
        self.inner.read().unwrap().committed_gen
    }

    /// Promote the current staged buffer to committed without starting a new
    /// pass. Use this in tests when the last render has finished but the
    /// double-buffer hasn't rotated yet (GPUI test scheduler runs renders
    /// on demand, not on a frame clock, so there's no automatic second pass).
    /// Without this, snapshot readers see data one render behind the actual
    /// UI state — which masks any regression that manifests *after* a single
    /// re-render. No-op when staged is empty so repeated calls are safe.
    pub fn flush(&self) {
        let mut state = self.inner.write().unwrap();
        let new = std::mem::take(&mut state.staged);
        if !new.is_empty() {
            state.committed = new;
            state.cold = false;
            state.committed_gen += 1;
            drop(state);
            self.commit_notify.notify_waiters();
        }
    }
}

impl GeometryProvider for BoundsRegistry {
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        self.inner.read().unwrap().committed.get(id).cloned()
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        self.inner
            .read()
            .unwrap()
            .committed
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Wakes on the next committed-buffer rotation (a fresh frame's data is
    /// readable). Callers must wrap in a timeout: a notification can land
    /// between their predicate check and this await, and GPUI only paints
    /// when something requests a frame.
    fn changed(&self) -> futures::future::BoxFuture<'static, ()> {
        let notify = self.commit_notify.clone();
        Box::pin(async move { notify.notified().await })
    }

    fn generation(&self) -> u64 {
        self.committed_generation()
    }
}

// Thread-local render-path stack used by `BoundsTracker` / `TransparentTracker`
// to record each widget's immediate tracked parent. Pushed on `prepaint` before
// recursing into children, popped after.
//
// This is single-threaded by construction: GPUI runs all render / layout /
// paint on the main thread, and tests use `TestAppContext` which also serializes
// work through a single dispatcher. The thread-local therefore always reflects
// the current render pass's path without any locking.
thread_local! {
    static RENDER_PATH: std::cell::RefCell<Vec<Arc<str>>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn current_parent() -> Option<Arc<str>> {
    RENDER_PATH.with(|p| p.borrow().last().cloned())
}

/// Clip a tracked element's layout bounds to the active content mask so the
/// recorded rectangle is the *visible* (and therefore clickable) region — not
/// the unclipped layout box. GPUI's mouse hit-test uses `bounds ∩ content_mask`
/// (`Window::hit_test`), so an element that overflows its scroll/panel clip is
/// only interactive within the visible part. Recording the unclipped box let
/// the PBT driver compute a click point (the geometric centre) that fell in the
/// overflowed, off-panel region where no hitbox exists, so the click silently
/// missed and bound operations (e.g. `navigation.focus`) never fired.
fn visible_bounds(
    bounds: gpui::Bounds<gpui::Pixels>,
    window: &gpui::Window,
) -> gpui::Bounds<gpui::Pixels> {
    bounds.intersect(&window.content_mask().bounds)
}

fn push_parent(id: Arc<str>) {
    RENDER_PATH.with(|p| p.borrow_mut().push(id));
}

fn pop_parent() {
    RENDER_PATH.with(|p| {
        p.borrow_mut().pop();
    });
}

/// Wrap an element so its computed bounds and metadata are recorded in `BoundsRegistry`
/// during prepaint.
///
/// The wrapper is transparent — it takes the same layout as its child and adds no
/// visual or interactive behavior.
pub fn tracked(
    el_id: impl Into<String>,
    child: AnyElement,
    registry: &BoundsRegistry,
    widget_type: &str,
    entity_id: Option<&str>,
    has_content: bool,
    displayed_text: Option<Arc<str>>,
) -> BoundsTracker {
    BoundsTracker {
        el_id: Arc::from(el_id.into()),
        registry: registry.clone(),
        widget_type: Arc::from(widget_type),
        entity_id: entity_id.map(Arc::from),
        has_content,
        displayed_text,
        focused: None,
        expected_size: SizeBounds::default(),
        child: Some(child),
    }
}

impl BoundsTracker {
    /// Declare an expected min/max size for this tracked element. Picked
    /// up by the layout invariant at check time — see
    /// [`holon_frontend::size_expectation`].
    pub fn with_expected_size(mut self, expected: SizeBounds) -> Self {
        self.expected_size = expected;
        self
    }

    /// Record whether this widget's focus handle held window focus at
    /// render time (focusable widgets only — see `ElementInfo::focused`).
    pub fn with_focused(mut self, focused: bool) -> Self {
        self.focused = Some(focused);
        self
    }
}

/// Transparent wrapper element that records its child's bounds into a
/// `BoundsRegistry`.
///
/// **Note**: this wrapper is *not* layout-transparent. It overrides the
/// child's style with `width: 100%; flex_grow: 1` to work around a specific
/// `live_block` use case. For general per-widget bounds tracking (fast UI
/// tests, observability) use `TransparentTracker` below, which returns the
/// child's layout id directly and adds no style of its own.
pub struct BoundsTracker {
    el_id: Arc<str>,
    registry: BoundsRegistry,
    widget_type: Arc<str>,
    entity_id: Option<Arc<str>>,
    has_content: bool,
    displayed_text: Option<Arc<str>>,
    focused: Option<bool>,
    expected_size: SizeBounds,
    child: Option<AnyElement>,
}

/// Truly layout-transparent wrapper that records its child's final bounds
/// into a `BoundsRegistry` during prepaint.
///
/// Unlike `BoundsTracker`, this does *not* create its own layout node — it
/// returns the child's `LayoutId` unchanged, so Taffy measures the child
/// exactly as if the tracker weren't there. This is the wrapper used by
/// `render::builders::tag()` for every widget.
///
/// Fields other than `widget_type` (`entity_id`, `has_content`) are left
/// defaulted because `tag()` is called for every builder and doesn't know
/// about entity identity or "has content" semantics — those are recorded by
/// specific builders (like `live_block`) on their own via `tracked()`.
pub struct TransparentTracker {
    el_id: Arc<str>,
    widget_type: Arc<str>,
    registry: BoundsRegistry,
    expected_size: SizeBounds,
    /// Optional entity binding for region-scoped queries against
    /// `BoundsRegistry`. `live_block` sets this so PBT generators can find
    /// which subtree of the rendered tree belongs to which panel (e.g.
    /// `block:default-left-sidebar`) without consulting ref-state predictions.
    entity_id: Option<Arc<str>>,
    child: Option<AnyElement>,
}

impl TransparentTracker {
    pub fn new(
        el_id: String,
        widget_type: &'static str,
        registry: BoundsRegistry,
        child: AnyElement,
    ) -> Self {
        Self {
            el_id: Arc::from(el_id),
            widget_type: Arc::from(widget_type),
            registry,
            expected_size: SizeBounds::default(),
            entity_id: None,
            child: Some(child),
        }
    }

    /// Bind an entity URI so region queries can find this subtree by
    /// `entity_id` (e.g. the `live_block` for the LeftSidebar binds itself
    /// to `block:default-left-sidebar`).
    pub fn with_entity_id(mut self, entity_id: impl Into<Arc<str>>) -> Self {
        self.entity_id = Some(entity_id.into());
        self
    }

    /// Declare an expected min/max size for this transparently-tracked
    /// element. See [`holon_frontend::size_expectation`].
    pub fn with_expected_size(mut self, expected: SizeBounds) -> Self {
        self.expected_size = expected;
        self
    }
}

impl IntoElement for TransparentTracker {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TransparentTracker {
    /// We return the *child's* LayoutId as our own. Taffy only allocates one
    /// layout node, and its bounds ARE the child's bounds.
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_id = self.child.as_mut().unwrap().request_layout(window, cx);
        (child_layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let parent_id = current_parent();
        let vis = visible_bounds(bounds, window);
        self.registry.record(
            self.el_id.to_string(),
            ElementInfo {
                x: f32::from(vis.origin.x),
                y: f32::from(vis.origin.y),
                width: f32::from(vis.size.width),
                height: f32::from(vis.size.height),
                widget_type: Arc::clone(&self.widget_type),
                entity_id: self.entity_id.clone(),
                has_content: false,
                parent_id,
                displayed_text: None,
                focused: None,
                expected_size: self.expected_size.clone(),
            },
        );
        push_parent(Arc::clone(&self.el_id));
        self.child.as_mut().unwrap().prepaint(window, cx);
        pop_parent();
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.as_mut().unwrap().paint(window, cx);
    }
}

impl IntoElement for BoundsTracker {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BoundsTracker {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_id = self.child.as_mut().unwrap().request_layout(window, cx);
        let mut style = gpui::Style::default();
        // Propagate width so children using w_full() resolve against
        // the BoundsTracker's parent rather than collapsing to zero.
        style.size.width = gpui::relative(1.0).into();
        style.flex_grow = 1.0;
        let wrapper_layout_id = window.request_layout(style, [child_layout_id], cx);
        (wrapper_layout_id, child_layout_id)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let parent_id = current_parent();
        let vis = visible_bounds(bounds, window);
        self.registry.record(
            self.el_id.to_string(),
            ElementInfo {
                x: f32::from(vis.origin.x),
                y: f32::from(vis.origin.y),
                width: f32::from(vis.size.width),
                height: f32::from(vis.size.height),
                widget_type: Arc::clone(&self.widget_type),
                entity_id: self.entity_id.clone(),
                has_content: self.has_content,
                parent_id,
                displayed_text: self.displayed_text.clone(),
                focused: self.focused,
                expected_size: self.expected_size.clone(),
            },
        );
        push_parent(Arc::clone(&self.el_id));
        self.child.as_mut().unwrap().prepaint(window, cx);
        pop_parent();
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.as_mut().unwrap().paint(window, cx);
    }
}
