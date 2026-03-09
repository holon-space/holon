//! GPUI GeometryProvider — reads from a shared BoundsRegistry populated during render.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{Bounds, Pixels};
use holon_frontend::geometry::{ElementRect, GeometryProvider};
use holon_frontend::theme::ThemeColors;

pub fn rgba8_to_gpui(c: holon_frontend::theme::Rgba8) -> gpui::Rgba {
    gpui::Rgba {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: c[3] as f32 / 255.0,
    }
}

/// Shared registry of element bounds, populated during GPUI render passes.
///
/// Also carries the active `ThemeColors` so builders can access theme
/// tokens via `ba.ctx.ext.theme()`.
#[derive(Clone)]
pub struct BoundsRegistry {
    inner: Arc<RwLock<HashMap<String, Bounds<Pixels>>>>,
    theme: Arc<ThemeColors>,
    /// Block ID of the left sidebar, set during screen layout render.
    /// Read by the title bar toggle button to persist open/closed state.
    sidebar_block_id: Arc<RwLock<Option<String>>>,
}

impl BoundsRegistry {
    pub fn new(theme: ThemeColors) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            theme: Arc::new(theme),
            sidebar_block_id: Arc::new(RwLock::new(None)),
        }
    }

    pub fn theme(&self) -> &ThemeColors {
        &self.theme
    }

    pub fn gpui_color(&self, rgba: holon_frontend::theme::Rgba8) -> gpui::Rgba {
        rgba8_to_gpui(rgba)
    }

    pub fn set_sidebar_block_id(&self, id: String) {
        *self.sidebar_block_id.write().unwrap() = Some(id);
    }

    pub fn sidebar_block_id(&self) -> Option<String> {
        self.sidebar_block_id.read().unwrap().clone()
    }

    /// Record the bounds of an element after layout.
    pub fn record(&self, id: String, bounds: Bounds<Pixels>) {
        self.inner.write().unwrap().insert(id, bounds);
    }

    /// Clear all recorded bounds (call at the start of each render pass).
    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }
}

impl GeometryProvider for BoundsRegistry {
    fn element_bounds(&self, id: &str) -> Option<ElementRect> {
        let map = self.inner.read().unwrap();
        map.get(id).map(|b| ElementRect {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        })
    }

    fn all_element_ids(&self) -> Vec<String> {
        self.inner.read().unwrap().keys().cloned().collect()
    }
}
