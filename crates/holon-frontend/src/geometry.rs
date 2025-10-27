//! Geometry provider trait for cross-frontend UI testing.
//!
//! Each frontend implements `GeometryProvider` to expose element bounds
//! by entity ID. The `GeometryDriver` (in holon-integration-tests) uses
//! these bounds to simulate mouse clicks via `enigo`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Bounding rectangle of a UI element in screen coordinates.
#[derive(Debug, Clone, Copy)]
pub struct ElementRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ElementRect {
    /// Center point of this rectangle.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Provides element bounds for geometry-based UI interaction.
///
/// Frontends implement this by querying their framework's layout system
/// for the bounds of elements tagged with entity IDs (via the annotator).
pub trait GeometryProvider: Send + Sync {
    /// Look up the bounding rectangle of an element by its entity ID.
    fn element_bounds(&self, id: &str) -> Option<ElementRect>;

    /// List all element IDs currently visible in the UI.
    fn all_element_ids(&self) -> Vec<String>;
}

/// Framework-agnostic shared bounds registry.
///
/// Frontends write element bounds here during render/layout passes.
/// Tests and drivers read from it via the `GeometryProvider` impl.
/// Works across threads (Arc + RwLock).
#[derive(Clone, Default)]
pub struct SharedBoundsRegistry {
    inner: Arc<RwLock<HashMap<String, ElementRect>>>,
}

impl SharedBoundsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the bounds of an element after layout.
    pub fn record(&self, id: String, rect: ElementRect) {
        self.inner.write().unwrap().insert(id, rect);
    }

    /// Clear all recorded bounds (call at the start of each render pass).
    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }
}

impl GeometryProvider for SharedBoundsRegistry {
    fn element_bounds(&self, id: &str) -> Option<ElementRect> {
        self.inner.read().unwrap().get(id).copied()
    }

    fn all_element_ids(&self) -> Vec<String> {
        self.inner.read().unwrap().keys().cloned().collect()
    }
}
