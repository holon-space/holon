//! Blinc GeometryProvider — wraps `query(id).bounds()` from blinc_layout selector API.
//!
//! Uses the global `query()` function which accesses `BlincContextState` singleton.
//! This means `element_info()` only works when called from the Blinc event loop thread.

use blinc_layout::prelude::{ElementHandle, ElementRegistry};
use holon_frontend::geometry::{ElementInfo, GeometryProvider};
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
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        let handle: ElementHandle<()> = ElementHandle::new(id, Arc::clone(&self.registry));
        let bounds = handle.bounds()?;
        Some(ElementInfo {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
            widget_type: "unknown".to_string(),
            entity_id: None,
            has_content: true,
        })
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        vec![]
    }
}
