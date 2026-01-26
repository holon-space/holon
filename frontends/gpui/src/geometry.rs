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
}

impl BoundsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BoundsState {
                staged: HashMap::new(),
                committed: HashMap::new(),
                seq: 0,
                cold: true,
            })),
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
        if !new.is_empty() {
            state.committed = new;
            state.cold = false;
        }
        state.seq = 0;
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
    static RENDER_PATH: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn current_parent() -> Option<String> {
    RENDER_PATH.with(|p| p.borrow().last().cloned())
}

fn push_parent(id: String) {
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
    displayed_text: Option<String>,
) -> BoundsTracker {
    BoundsTracker {
        el_id: el_id.into(),
        registry: registry.clone(),
        widget_type: widget_type.to_string(),
        entity_id: entity_id.map(|s| s.to_string()),
        has_content,
        displayed_text,
        child: Some(child),
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
    el_id: String,
    registry: BoundsRegistry,
    widget_type: String,
    entity_id: Option<String>,
    has_content: bool,
    displayed_text: Option<String>,
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
    el_id: String,
    widget_type: &'static str,
    registry: BoundsRegistry,
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
            el_id,
            widget_type,
            registry,
            child: Some(child),
        }
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_id = self.child.as_mut().unwrap().request_layout(window, cx);
        (child_layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let parent_id = current_parent();
        self.registry.record(
            self.el_id.clone(),
            ElementInfo {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                widget_type: self.widget_type.to_string(),
                entity_id: None,
                has_content: false,
                parent_id,
                displayed_text: None,
            },
        );
        push_parent(self.el_id.clone());
        self.child.as_mut().unwrap().prepaint(window, cx);
        pop_parent();
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _child_layout_id: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let parent_id = current_parent();
        self.registry.record(
            self.el_id.clone(),
            ElementInfo {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                widget_type: self.widget_type.clone(),
                entity_id: self.entity_id.clone(),
                has_content: self.has_content,
                parent_id,
                displayed_text: self.displayed_text.clone(),
            },
        );
        push_parent(self.el_id.clone());
        self.child.as_mut().unwrap().prepaint(window, cx);
        pop_parent();
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _child_layout_id: &mut LayoutId,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.as_mut().unwrap().paint(window, cx);
    }
}
