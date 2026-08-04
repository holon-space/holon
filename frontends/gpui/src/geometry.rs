//! GPUI GeometryProvider — reads from a shared BoundsRegistry populated during
//! render.
//!
//! [`TransparentTracker`] records the computed bounds of its child into the
//! `BoundsRegistry` during the prepaint phase. Use [`tracked()`] to wrap any
//! element that should be locatable for click-based PBT testing.
//!
//! # The tracked-widget contract
//!
//! **A tracker observes layout; it never contributes any.** It returns its
//! child's `LayoutId` as its own, so Taffy measures the child exactly as if
//! the tracker weren't there, and the recorded rect is a measurement of the
//! widget rather than of the instrument.
//!
//! The obligation this puts on widgets: **a widget that must fill its row
//! says so itself** — `w_full()` / `flex_1()` on the element it hands to
//! `tracked()`, not inherited from the wrapper. `rendered_text` and
//! `editable_text` do; `selectable` deliberately does not, because a bullet's
//! click region is the bullet.
//!
//! Enforced by `tests/tracked_layout_neutrality.rs` (fast-UI PBT over the
//! shipped block-row shape) and, on live windowed runs, by the
//! `expected-size-satisfied` sub-check of `inv-frontend-bounds-rendered`
//! reading the [`SizeBounds`] that `selectable` declares.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use gpui::AnyElement;
use gpui::App;
use gpui::Bounds;
use gpui::Element;
use gpui::ElementId;
use gpui::GlobalElementId;
use gpui::InspectorElementId;
use gpui::IntoElement;
use gpui::LayoutId;
use gpui::Pixels;
use gpui::Window;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::geometry::VmNode;
use holon_frontend::size_expectation::SizeBounds;

/// Shared registry of element metadata, populated during GPUI render passes.
///
/// Double-buffered: writes during a render pass go to `staged`, reads come from
/// `committed`. At the start of each render pass, `begin_pass()` atomically
/// moves the previous `staged` into `committed` and resets `staged` for the new
/// pass.
///
/// This gives readers a consistent snapshot of the PREVIOUS fully-completed
/// render:
///   - Frame N writes populate `staged`.
///   - Frame N+1's `begin_pass()` moves frame N's data into `committed`.
///   - Readers see frame N's data until frame N+2 arrives.
///
/// Note: readers see data that is one frame behind. This is fine because GPUI
/// renders continuously and tests wait (sleep + settle) between state
/// mutations, so `committed` reflects the stable "last complete render" by the
/// time tests read. If the UI becomes empty in a re-render, that propagates to
/// `committed` after the next pass, so empty-UI regressions are detected.
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

    /// Begin a new render pass. Promotes staged → committed (if staged has
    /// data), resets staged for the new pass, and resets the per-pass
    /// sequence counter.
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

    /// Number of committed-buffer rotations so far (see
    /// [`BoundsState::committed_gen`]). Strictly increases on each
    /// non-empty `begin_pass`/`flush`.
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

    fn clone_box(&self) -> Box<dyn GeometryProvider> {
        Box::new(self.clone())
    }
}

/// [`GeometryProvider`] over a [`BoundsRegistry`] that promotes the staged
/// buffer before every read. A window that paints once and then goes idle
/// (iOS) leaves the last frame's bounds in `staged` forever — no next
/// `begin_pass` ever rotates them — so an idle-window reader (the MCP
/// driver) would see stale/empty `committed`. MCP reads arrive when the app
/// is quiescent (no render pass in flight), so an on-demand `flush` commits
/// exactly the last complete frame. Do NOT hand this wrapper to a
/// render-concurrent reader: a mid-pass flush splits one frame's writes
/// across two rotations.
#[derive(Clone)]
pub struct FlushOnReadGeometry(pub BoundsRegistry);

impl GeometryProvider for FlushOnReadGeometry {
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        self.0.flush();
        GeometryProvider::element_info(&self.0, id)
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        self.0.flush();
        GeometryProvider::all_elements(&self.0)
    }

    fn changed(&self) -> futures::future::BoxFuture<'static, ()> {
        GeometryProvider::changed(&self.0)
    }

    fn generation(&self) -> u64 {
        GeometryProvider::generation(&self.0)
    }

    fn clone_box(&self) -> Box<dyn GeometryProvider> {
        Box::new(self.clone())
    }
}

// Thread-local render-path stack used by `TransparentTracker`
// to record each widget's immediate tracked parent. Pushed on `prepaint` before
// recursing into children, popped after.
//
// This is single-threaded by construction: GPUI runs all render / layout /
// paint on the main thread, and tests use `TestAppContext` which also
// serializes work through a single dispatcher. The thread-local therefore
// always reflects the current render pass's path without any locking.
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

/// Wrap an element so its computed bounds and metadata are recorded in
/// `BoundsRegistry` during prepaint.
///
/// Layout-transparent, per the module's tracked-widget contract: the wrapper
/// takes the child's own `LayoutId` and adds no style, no visual, and no
/// interactive behavior.
pub fn tracked(
    el_id: impl Into<String>,
    child: AnyElement,
    registry: &BoundsRegistry,
    widget_type: &str,
    entity_id: Option<&str>,
    has_content: bool,
    displayed_text: Option<Arc<str>>,
) -> TransparentTracker {
    TransparentTracker {
        el_id: Arc::from(el_id.into()),
        widget_type: Arc::from(widget_type),
        registry: registry.clone(),
        expected_size: SizeBounds::default(),
        entity_id: entity_id.map(Arc::from),
        has_content,
        displayed_text,
        focused: None,
        styled_runs: None,
        opacity: None,
        vm_node: None,
        child: Some(child),
    }
}

/// Layout-transparent wrapper that records its child's final bounds into a
/// `BoundsRegistry` during prepaint.
///
/// It does *not* create its own layout node — it returns the child's
/// `LayoutId` unchanged, so Taffy measures the child exactly as if the
/// tracker weren't there. Both entry points produce it: `tracked()` for
/// builders that know an entity identity, and `render::builders::tag()` for
/// blanket per-widget observability.
///
/// `tag()` leaves the identity fields (`entity_id`, `has_content`, …)
/// defaulted because it runs for every builder and knows none of them; the
/// specific builders record those through `tracked()`.
pub struct TransparentTracker {
    el_id: Arc<str>,
    widget_type: Arc<str>,
    registry: BoundsRegistry,
    expected_size: SizeBounds,
    /// Whether this element has visible content — see `ElementInfo`.
    has_content: bool,
    /// Whether this widget's focus handle held window focus at render time
    /// (focusable widgets only — see `ElementInfo::focused`).
    focused: Option<bool>,
    /// The read-mode styled-run fingerprint this widget painted (the runs
    /// handed to `StyledText::with_highlights`). Only the mark-styling path
    /// sets it; a plain-text render leaves it `None`. Read back by
    /// `inv-paint-text-styling` via the `GeometryProvider`.
    styled_runs: Option<Arc<[holon_api::StyledRun]>>,
    /// Optional entity binding for region-scoped queries against
    /// `BoundsRegistry`. `live_block` sets this so PBT generators can find
    /// which subtree of the rendered tree belongs to which panel (e.g.
    /// `block:default-left-sidebar`) without consulting ref-state predictions.
    entity_id: Option<Arc<str>>,
    /// What the wrapped element paints, when it is a leaf that paints text
    /// (the tree row's disclosure glyph, say) rather than a container.
    displayed_text: Option<Arc<str>>,
    /// The alpha the wrapped element declares — see [`ElementInfo::opacity`].
    opacity: Option<f32>,
    /// The view-model node this tracker wraps — see [`VmNode`]. Set by the
    /// node-dispatch `tag_node()`, which is the only site that holds the node.
    vm_node: Option<VmNode>,
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
            has_content: false,
            focused: None,
            styled_runs: None,
            entity_id: None,
            displayed_text: None,
            opacity: None,
            vm_node: None,
            child: Some(child),
        }
    }

    /// Record whether this widget's focus handle held window focus at
    /// render time (focusable widgets only — see `ElementInfo::focused`).
    pub fn with_focused(mut self, focused: bool) -> Self {
        self.focused = Some(focused);
        self
    }

    /// Record the read-mode styled-run fingerprint this widget painted.
    /// See the field's doc for who reads it back.
    pub fn with_styled_runs(mut self, runs: Vec<holon_api::StyledRun>) -> Self {
        self.styled_runs = Some(Arc::from(runs));
        self
    }

    /// Record the text the wrapped element paints, so invariants can judge
    /// *which* glyph a control drew (e.g. a chevron's direction).
    pub fn with_displayed_text(mut self, text: impl Into<Arc<str>>) -> Self {
        self.displayed_text = Some(text.into());
        self
    }

    /// Record the wrapped element's paint alpha, so invariants can tell a
    /// visible control from one that is laid out but transparent.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    /// Declare which view-model node this tracker was created for, so
    /// `describe_ui` can join that node to THIS rect instead of to a sibling
    /// element that merely renders the same entity.
    pub fn with_vm_node(mut self, entity: Option<&str>) -> Self {
        self.vm_node = Some(VmNode {
            tag: Arc::clone(&self.widget_type),
            entity: entity.map(Arc::from),
        });
        self
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
                has_content: self.has_content,
                parent_id,
                displayed_text: self.displayed_text.clone(),
                focused: self.focused,
                styled_runs: self.styled_runs.clone(),
                opacity: self.opacity,
                expected_size: self.expected_size.clone(),
                vm_node: self.vm_node.clone(),
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

#[cfg(test)]
mod tests {
    use holon_frontend::geometry::GeometryProvider;

    use super::*;

    fn elem(entity: &str) -> ElementInfo {
        ElementInfo {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 20.0,
            widget_type: Arc::from("live_block"),
            entity_id: Some(Arc::from(entity)),
            has_content: true,
            parent_id: None,
            displayed_text: None,
            focused: None,
            styled_runs: None,
            opacity: None,
            expected_size: SizeBounds::default(),
            vm_node: None,
        }
    }

    /// Promotion-timing invariant that `click_entity`'s retry-until-committed
    /// depends on: once cold start is over, a freshly `record()`ed element is
    /// invisible to plain committed reads until the next `begin_pass`/`flush`
    /// — yet a `FlushOnReadGeometry` read promotes it immediately. A
    /// single-shot click on such a just-rendered `:__virtual:` slot would miss
    /// on the plain read (the dogfood #3 race); the driver must either read
    /// flush-on-read or retry across a commit to see it.
    #[test]
    fn fresh_record_invisible_until_promoted_but_flush_on_read_sees_it() {
        let reg = BoundsRegistry::new();
        // Leave cold start: the first non-empty rotation clears `cold`, after
        // which records hit staged only (real double-buffering).
        reg.record("render-entity-block:warmup".into(), elem("block:warmup"));
        reg.begin_pass();
        assert!(
            GeometryProvider::element_info(&reg, "render-entity-block:warmup").is_some(),
            "warmup element must be committed after its begin_pass rotation"
        );

        // A brand-new creation slot appears in THIS frame (staged only).
        let slot = "render-entity-block:__virtual:default-main-panel";
        reg.record(slot.into(), elem("block:__virtual:default-main-panel"));

        // Plain committed read races the promotion: not yet visible.
        assert!(
            GeometryProvider::element_info(&reg, slot).is_none(),
            "post-cold record must stay staged until the next begin_pass/flush — this is the race \
             that made the single-shot click fail"
        );

        // Flush-on-read promotes the last frame's staged bounds on demand, so
        // the freshly-rendered slot becomes clickable without a second pass.
        let flush = FlushOnReadGeometry(reg.clone());
        let gen_before = GeometryProvider::generation(&reg);
        assert!(
            GeometryProvider::element_info(&flush, slot).is_some(),
            "flush-on-read must promote the just-rendered creation slot"
        );
        assert_eq!(
            GeometryProvider::generation(&reg),
            gen_before + 1,
            "flush must rotate committed_gen so retry loops waking on `changed()`/generation \
             observe the new frame"
        );
    }
}
