//! Blinc GeometryProvider — wraps `query(id).bounds()` from blinc_layout selector API.
//!
//! Uses the global `query()` function which accesses `BlincContextState` singleton.
//! This means `element_bounds()` only works when called from the Blinc event loop thread.

use blinc_layout::prelude::{ElementHandle, ElementRegistry};
use holon_frontend::geometry::{ElementRect, GeometryProvider};
use std::sync::Arc;

pub struct BlincGeometry {
    registry: Arc<ElementRegistry>,
}

impl BlincGeometry {
    pub fn new(registry: Arc<ElementRegistry>) -> Self {
        Self { registry }
    }
}

impl GeometryProvider for BlincGeometry {
    fn element_bounds(&self, id: &str) -> Option<ElementRect> {
        let handle: ElementHandle<()> = ElementHandle::new(id, Arc::clone(&self.registry));
        let bounds = handle.bounds()?;
        Some(ElementRect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        })
    }

    fn all_element_ids(&self) -> Vec<String> {
        vec![]
    }
}
