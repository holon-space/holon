use super::prelude::*;
use crate::render_context::{AvailableSpace, LayoutHint};
use crate::view_model::DrawerMode;

pub const DEFAULT_DRAWER_WIDTH: f32 = 260.0;

/// A collapsible drawer wrapping a single child.
///
/// ```text
/// drawer(col("id"), live_block())
/// drawer("sidebar-id", live_block("sidebar-id"), #{mode: "overlay"})
/// ```
///
/// Positional args:
/// - `0`: block id (usually `col("id")` resolved against the current row, or a
///   literal string for hard-coded layout) — identifies which block's
///   collapse/expand state this drawer controls.
/// - `1`: the child render expression.
///
/// Named args:
/// - `mode`: `"shrink"` (default) or `"overlay"`. Shrink drawers push siblings
///   aside when open; overlay drawers float above siblings without affecting
///   their size.
/// - `width`: drawer width in logical pixels (default `DEFAULT_DRAWER_WIDTH`).
///   Overlay drawers use this as their panel width but claim 0px in flow layout.
holon_macros::widget_builder! {
    raw fn drawer(ba: BA<'_>) -> ViewModel {
        let block_id = ba
            .args
            .get_positional_string(0)
            .or_else(|| {
                ba.ctx
                    .row()
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        let mode = ba
            .args
            .get_string("mode")
            .map(DrawerMode::from_str)
            .unwrap_or_default();

        let width = ba
            .args
            .get_f64("width")
            .map(|v| v as f32)
            .unwrap_or(DEFAULT_DRAWER_WIDTH);

        // Give the child its own space budget equal to the drawer width so
        // that if_space() / pick_active_variant() inside the drawer evaluate
        // against the drawer's actual allocation, not the parent's width.
        let child_ctx = match ba.ctx.available_space {
            Some(parent) => ba.ctx.with_available_space(AvailableSpace {
                width_px: width,
                width_physical_px: width * parent.scale_factor,
                ..parent
            }),
            None => ba.ctx.clone(),
        };

        let child = match ba.args.positional_exprs.get(1) {
            Some(expr) => (ba.interpret)(expr, &child_ctx),
            None => ViewModel::empty(),
        };

        let layout_hint = match mode {
            // Overlay drawers float above siblings — zero flow footprint.
            DrawerMode::Overlay => LayoutHint::Fixed { px: 0.0 },
            // Shrink drawers always reserve their full width in the partition,
            // even when closed. The GPUI renderer collapses to 0 inside that slot.
            DrawerMode::Shrink => LayoutHint::Fixed { px: width },
        };

        ViewModel::drawer(block_id, mode, width, child).with_layout_hint(layout_hint)
    }
}
