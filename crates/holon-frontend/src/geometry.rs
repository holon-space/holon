//! Geometry provider trait for cross-frontend UI testing.
//!
//! Each frontend implements `GeometryProvider` to expose element metadata
//! by element ID. The `GeometryDriver` (in holon-integration-tests) uses
//! these bounds to simulate mouse clicks via `enigo`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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
    /// Used as a last-resort fallback in click-target lookup when the
    /// canonical `render-entity-{id}` / `selectable-{id}` el_ids miss —
    /// e.g. a builder that `tracked()`'d under a non-standard prefix.
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
}

impl SharedBoundsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record element metadata after layout.
    pub fn record(&self, id: String, info: ElementInfo) {
        self.inner.write().unwrap().insert(id, info);
    }

    /// Clear all recorded elements (call at the start of each render pass).
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

/// Canonical element-id for a View Mode Switcher mode button.
///
/// Every frontend's VMS builder must tag each mode button with this string.
/// Tests and drivers look up the same string to locate a button for click
/// dispatch. Stable across renders: same `(block_id, mode)` → same string.
pub fn vms_button_id_for(block_id: &str, mode: &str) -> String {
    format!("vms_button::{block_id}::{mode}")
}
