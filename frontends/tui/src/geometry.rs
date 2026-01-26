//! Cell-based [`GeometryProvider`] backed by the TUI's per-frame
//! [`RenderRegistry`].
//!
//! The TUI renderer already discovers every entity-bearing region during the
//! render walk and stores it in `state.last_registry: Arc<Mutex<RenderRegistry>>`
//! (see `app_main`). [`TuiGeometry`] wraps that same `Arc` so PBT consumers
//! (drivers, inv14, the readiness gate) get the latest paint via the standard
//! `holon_frontend::geometry::GeometryProvider` interface.
//!
//! Cell→pixel translation uses fixed [`CELL_W`] / [`CELL_H`] constants so the
//! `f32` rectangles in [`ElementInfo`] stay self-consistent with whatever
//! screenshot painter the harness uses
//! (see `crate::screenshot::OffscreenBufferBackend`).

use std::sync::{Arc, Mutex};

use holon_frontend::geometry::{ElementInfo, GeometryProvider};

use crate::render::{RenderRegistry, SelectableRegion};

/// Pixel width of a single character cell when projecting to
/// `ElementInfo` coordinates.
pub const CELL_W: f32 = 8.0;

/// Pixel height of a single character cell when projecting to
/// `ElementInfo` coordinates.
pub const CELL_H: f32 = 16.0;

/// Cell-based [`GeometryProvider`] that views the TUI's per-frame
/// [`RenderRegistry`] as `(entity_id, ElementInfo)` pairs.
///
/// The wrapped `Arc<Mutex<RenderRegistry>>` is shared with the renderer
/// (`AppMain` / `CapturingApp`); each frame's `app_render` swaps in a fresh
/// registry, and `TuiGeometry` reads whatever is current at lookup time.
#[derive(Clone, Default)]
pub struct TuiGeometry {
    inner: Arc<Mutex<RenderRegistry>>,
}

impl TuiGeometry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap an existing shared registry. Use when the renderer needs to write
    /// into the same `Arc<Mutex<_>>` (e.g. `state.last_registry` in
    /// `AppMain`).
    pub fn from_shared(inner: Arc<Mutex<RenderRegistry>>) -> Self {
        Self { inner }
    }

    /// Replace the wrapped registry's contents in-place. Called by the
    /// renderer at the end of each render pass.
    pub fn install(&self, registry: RenderRegistry) {
        *self.inner.lock().unwrap() = registry;
    }

    /// Borrow access to the underlying shared registry. The renderer uses this
    /// to wire `state.last_registry` and `TuiGeometry` to the same allocation.
    pub fn shared(&self) -> Arc<Mutex<RenderRegistry>> {
        self.inner.clone()
    }
}

fn to_element_info(region: &SelectableRegion) -> ElementInfo {
    let cols = region.cols.max(1);
    let rows = region.rows.max(1);
    ElementInfo {
        x: region.start_col as f32 * CELL_W,
        y: region.start_row as f32 * CELL_H,
        width: cols as f32 * CELL_W,
        height: rows as f32 * CELL_H,
        widget_type: region.widget_type.clone(),
        entity_id: Some(region.entity_id.clone()),
        // `has_content` mirrors GPUI's "this tracked element actually got laid
        // out" semantic: any region that completed a render pass with non-zero
        // dimensions is content-bearing. The readiness gate combines this with
        // `entity_id.is_some()` (always true here, since we only register
        // entity-bearing widgets) so it fires once the TUI has painted any
        // tracked row. Inline-text staleness invariants read
        // `displayed_text` directly, separate from this flag.
        has_content: region.rows > 0 && region.cols > 0,
        parent_id: None,
        displayed_text: region.displayed_text.clone(),
    }
}

impl GeometryProvider for TuiGeometry {
    fn element_info(&self, id: &str) -> Option<ElementInfo> {
        self.inner
            .lock()
            .unwrap()
            .selectables
            .iter()
            .find(|r| r.entity_id == id)
            .map(to_element_info)
    }

    fn all_elements(&self) -> Vec<(String, ElementInfo)> {
        self.inner
            .lock()
            .unwrap()
            .selectables
            .iter()
            .map(|r| (r.entity_id.clone(), to_element_info(r)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{SelectableKind, SelectableRegion};

    fn make_region(
        entity_id: &str,
        start_row: usize,
        start_col: usize,
        rows: usize,
        cols: usize,
        widget_type: &str,
        displayed_text: Option<&str>,
    ) -> SelectableRegion {
        SelectableRegion {
            entity_id: entity_id.to_string(),
            intent: None,
            kind: SelectableKind::Block,
            region: 0,
            editable: None,
            start_row,
            start_col,
            rows,
            cols,
            widget_type: widget_type.to_string(),
            displayed_text: displayed_text.map(|s| s.to_string()),
        }
    }

    #[test]
    fn all_elements_translates_cell_coords_to_pixels() {
        let geometry = TuiGeometry::new();
        geometry.install(RenderRegistry {
            selectables: vec![
                make_region("alpha", 0, 0, 1, 20, "selectable", Some("Files")),
                make_region("beta", 3, 4, 2, 16, "live_block", Some("hello")),
            ],
        });

        let elements = geometry.all_elements();
        assert_eq!(elements.len(), 2);

        let alpha = &elements.iter().find(|(id, _)| id == "alpha").unwrap().1;
        assert_eq!(alpha.x, 0.0);
        assert_eq!(alpha.y, 0.0);
        assert_eq!(alpha.width, 20.0 * CELL_W);
        assert_eq!(alpha.height, CELL_H);
        assert_eq!(alpha.widget_type, "selectable");
        assert_eq!(alpha.entity_id.as_deref(), Some("alpha"));
        assert!(alpha.has_content);

        let beta = &elements.iter().find(|(id, _)| id == "beta").unwrap().1;
        assert_eq!(beta.x, 4.0 * CELL_W);
        assert_eq!(beta.y, 3.0 * CELL_H);
        assert_eq!(beta.width, 16.0 * CELL_W);
        assert_eq!(beta.height, 2.0 * CELL_H);
        assert!(beta.has_content);
    }

    #[test]
    fn zero_rows_means_no_content() {
        let geometry = TuiGeometry::new();
        geometry.install(RenderRegistry {
            selectables: vec![
                // rows=0 represents a slot that was reserved but the inner
                // render returned 0 rows — nothing painted, so no content.
                make_region("placeholder", 0, 0, 0, 10, "selectable", None),
                make_region("zero-cols", 1, 0, 1, 0, "selectable", Some("hello")),
            ],
        });

        for (_, info) in geometry.all_elements() {
            assert!(
                !info.has_content,
                "{:?} should not be content-bearing",
                info
            );
        }
    }
}
