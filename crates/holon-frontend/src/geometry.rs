//! Geometry provider trait for cross-frontend UI testing.
//!
//! Each frontend implements `GeometryProvider` to expose element metadata
//! by element ID. The `GeometryDriver` (in holon-integration-tests) uses
//! these bounds to simulate mouse clicks via `enigo`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::size_expectation::SizeBounds;

/// Metadata about a rendered UI element: bounds, widget type, entity binding.
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Widget type name: "render_entity", "editable_text", "selectable", etc.
    pub widget_type: String,
    /// The entity URI this element represents (if data-bound).
    pub entity_id: Option<String>,
    /// Whether this element has visible content (false for empty containers).
    pub has_content: bool,
    /// Id of this widget's immediate tracked parent, if any. `None` at the
    /// root of the tracked tree. Populated by the GPUI `TransparentTracker`
    /// via a thread-local render-path stack, used by fast-UI layout
    /// invariants to rebuild the tree without a parallel view-model walk.
    pub parent_id: Option<String>,
    /// The text the widget actually puts on screen at the moment it was
    /// recorded. For `editable_text` this is the live `InputState` value;
    /// for plain `text` builders it's the resolved prop. `None` for
    /// containers and other widgets that don't carry textual content.
    /// Used by PBT invariants to detect UI staleness — e.g. the on-screen
    /// text of an `editable_text` failing to follow the SQL `block.content`
    /// after `split_block` / `join_block`.
    pub displayed_text: Option<String>,
    /// Algebraic declaration of the widget's expected min/max size. The
    /// PBT layout invariant evaluates this against `(width, height)` and
    /// fails on out-of-bounds renders. Defaults to "unconstrained" (all
    /// `Free`) so widgets opt in incrementally; see
    /// [`crate::size_expectation`] for the AST and ergonomics.
    pub expected_size: SizeBounds,
}

impl ElementInfo {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn area(&self) -> f32 {
        self.width * self.height
    }
}

/// Provides element metadata for geometry-based UI interaction and assertions.
///
/// Frontends implement this by querying their framework's layout system
/// for the bounds and metadata of elements tagged with entity IDs.
pub trait GeometryProvider: Send + Sync {
    /// Look up element metadata by its element ID.
    fn element_info(&self, id: &str) -> Option<ElementInfo>;

    /// All tracked elements with their metadata.
    fn all_elements(&self) -> Vec<(String, ElementInfo)>;

    /// Find any tracked element whose `entity_id` matches.
    ///
    /// ALLOW(fallback): documented last-resort lookup chain — `click_entity`
    /// tries canonical el_id prefixes first; this scans `all_elements` only
    /// when a builder `tracked()`'d under a non-standard prefix.
    /// Used as a last-resort lookup in click-target resolution when the
    /// canonical `render-entity-{id}` / `selectable-{id}` el_ids miss.
    fn find_by_entity_id(&self, entity_id: &str) -> Option<ElementInfo> {
        self.all_elements()
            .into_iter()
            .find(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
            .map(|(_, info)| info)
    }
}

/// Framework-agnostic shared bounds registry.
///
/// Frontends write element metadata here during render/layout passes.
/// Tests and drivers read from it via the `GeometryProvider` impl.
/// Works across threads (Arc + RwLock).
#[derive(Clone, Default)]
pub struct SharedBoundsRegistry {
    inner: Arc<RwLock<HashMap<String, ElementInfo>>>,
    /// Inter-pass stability tracker — survives `clear()`. Maps
    /// `(widget_type, entity_id)` to the last `el_id` seen for that pair.
    /// When a pair appears under a *different* el_id in a later pass, GPUI
    /// drops any registered click handler from the previous render — a
    /// silent-handler-loss class of bug that's nearly invisible from logs.
    /// We emit a one-shot warning per migration so the next render exposes
    /// the instability before tests start blaming "click didn't fire."
    historical: Arc<RwLock<HashMap<(String, String), String>>>,
}

impl SharedBoundsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record element metadata after layout. Emits a warning on widget-id
    /// instability (same widget_type+entity_id pair previously seen under a
    /// different `el_id`) — see the `historical` field doc for why.
    pub fn record(&self, id: String, info: ElementInfo) {
        if let Some(eid) = info.entity_id.as_deref() {
            let key = (info.widget_type.clone(), eid.to_string());
            let mut hist = self.historical.write().unwrap();
            match hist.get(&key) {
                Some(prev) if prev != &id => {
                    tracing::warn!(
                        target: "geometry.stability",
                        "widget-id instability: widget_type={:?} entity_id={:?} \
                         previous el_id={:?} new el_id={:?} — \
                         GPUI handlers registered on the previous element are now orphaned",
                        info.widget_type, eid, prev, id,
                    );
                    eprintln!(
                        "[geometry.stability WARN] widget_type={:?} entity_id={:?} \
                         previous el_id={:?} new el_id={:?}",
                        info.widget_type, eid, prev, id,
                    );
                    hist.insert(key, id.clone());
                }
                Some(_) => { /* stable — same el_id */ }
                None => {
                    hist.insert(key, id.clone());
                }
            }
        }
        self.inner.write().unwrap().insert(id, info);
    }

    /// Clear all recorded elements (call at the start of each render pass).
    /// Does NOT clear the inter-pass `historical` tracker.
    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }
}

impl GeometryProvider for SharedBoundsRegistry {
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        self.inner.read().unwrap().get(id).cloned()
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Frontend-agnostic [`crate::size_expectation::EvalCtx`] over any
/// [`GeometryProvider`]. Constructed per element under inspection — the
/// `Self_(_)` / `Parent(_)` / `Child(_)` accessors are scoped to *this*
/// element, not the provider as a whole.
///
/// Snapshots the provider once on construction so `eval` doesn't hold the
/// registry lock across recursive evaluation.
pub struct ProviderEvalCtx {
    elements: HashMap<String, ElementInfo>,
    self_id: String,
    children: Vec<String>,
    viewport_w: Option<f32>,
    viewport_h: Option<f32>,
}

impl ProviderEvalCtx {
    /// Build a context for evaluating the `expected_size` of `self_id`.
    /// Pass `viewport` if the frontend can supply window dims; otherwise
    /// `Expr::Viewport(_)` evaluates to `None`.
    pub fn for_element(
        provider: &dyn GeometryProvider,
        self_id: impl Into<String>,
        viewport: Option<(f32, f32)>,
    ) -> Self {
        let self_id = self_id.into();
        let elements: HashMap<String, ElementInfo> = provider.all_elements().into_iter().collect();
        let children: Vec<String> = elements
            .iter()
            .filter(|(_, info)| info.parent_id.as_deref() == Some(self_id.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        let (viewport_w, viewport_h) = match viewport {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };
        Self {
            elements,
            self_id,
            children,
            viewport_w,
            viewport_h,
        }
    }

    /// Construct from an already-collected vector of elements (avoids a
    /// double `all_elements()` call when the caller has the data in hand —
    /// e.g. inv-frontend-bounds-rendered iterates the same set).
    pub fn from_snapshot(
        elements: &[(String, ElementInfo)],
        self_id: impl Into<String>,
        viewport: Option<(f32, f32)>,
    ) -> Self {
        let self_id = self_id.into();
        let map: HashMap<String, ElementInfo> = elements.iter().cloned().collect();
        let children: Vec<String> = elements
            .iter()
            .filter(|(_, info)| info.parent_id.as_deref() == Some(self_id.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        let (viewport_w, viewport_h) = match viewport {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };
        Self {
            elements: map,
            self_id,
            children,
            viewport_w,
            viewport_h,
        }
    }

    fn dim(info: &ElementInfo, axis: crate::size_expectation::Axis) -> f32 {
        match axis {
            crate::size_expectation::Axis::X => info.width,
            crate::size_expectation::Axis::Y => info.height,
        }
    }
}

impl crate::size_expectation::EvalCtx for ProviderEvalCtx {
    fn self_observed(&self, axis: crate::size_expectation::Axis) -> Option<f32> {
        self.elements.get(&self.self_id).map(|i| Self::dim(i, axis))
    }
    fn child_observed(
        &self,
        child_el_id: &str,
        axis: crate::size_expectation::Axis,
    ) -> Option<f32> {
        self.elements.get(child_el_id).map(|i| Self::dim(i, axis))
    }
    fn children_observed(&self, axis: crate::size_expectation::Axis) -> Vec<f32> {
        self.children
            .iter()
            .filter_map(|id| self.elements.get(id))
            .map(|i| Self::dim(i, axis))
            .collect()
    }
    fn child_count(&self) -> usize {
        self.children.len()
    }
    fn parent_observed(&self, axis: crate::size_expectation::Axis) -> Option<f32> {
        let pid = self
            .elements
            .get(&self.self_id)
            .and_then(|i| i.parent_id.as_deref())?;
        self.elements.get(pid).map(|i| Self::dim(i, axis))
    }
    fn viewport(&self, axis: crate::size_expectation::Axis) -> Option<f32> {
        match axis {
            crate::size_expectation::Axis::X => self.viewport_w,
            crate::size_expectation::Axis::Y => self.viewport_h,
        }
    }
}

/// Canonical element-id for a View Mode Switcher mode button.
///
/// Every frontend's VMS builder must tag each mode button with this string.
/// Tests and drivers look up the same string to locate a button for click
/// dispatch. Stable across renders: same `(block_id, mode)` → same string.
pub fn vms_button_id_for(block_id: &str, mode: &str) -> String {
    format!("vms_button::{block_id}::{mode}")
}

/// Canonical element-id for a drawer's open/close toggle area.
///
/// The drawer builder must tag its clickable toggle widget with this
/// string. The layout PBT's `ToggleDrawer` transition synthesises a
/// click on this id via `UserDriver::click_entity`, the same way a real
/// user would tap the toggle.
pub fn drawer_toggle_id_for(block_id: &str) -> String {
    format!("drawer_toggle::{block_id}")
}

/// Canonical element-id for an outline/tree row's expand chevron.
///
/// The frontend's `expand_toggle` builder must tag its clickable chevron
/// with this string in the bounds registry. The layout PBT's
/// `ToggleCollapse` transition clicks this id to flip the row's
/// `expanded` `Mutable<bool>`, exercising the same path a real user's
/// chevron tap would.
pub fn expand_toggle_id_for(target_id: &str) -> String {
    format!("expand_toggle::{target_id}")
}
