use holon_frontend::drawer_toggle_id_for;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::DrawerMode;

use super::prelude::*;
use crate::geometry::TransparentTracker;

/// Width of the clickable drawer-toggle handle. Stays visible in the
/// closed state so a user (or the layout PBT's `ToggleDrawer`
/// transition) can re-open the drawer. Shared between `drawer::render`
/// and the columns special-case for first/last shrink drawers.
pub(super) const DRAWER_TOGGLE_WIDTH: f32 = 12.0;

/// The bounds-tracked clickable toggle widget for `block_id`'s drawer.
/// Both renderers use this — the variant-specific framing (where the
/// toggle sits relative to the panel, how the overlay anchors it) lives
/// at the call site; the toggle widget itself is shared.
///
/// `el_id_suffix` differentiates the GPUI element id between call
/// sites (a single drawer can be rendered via either `drawer::render`
/// or the columns shrink fast-path, but never both simultaneously, so
/// the suffix only needs to be unique within one render pass).
pub(super) fn drawer_toggle_widget(
    block_id: &str,
    el_id_suffix: &str,
    mode: DrawerMode,
    ctx: &GpuiRenderContext,
) -> impl IntoElement {
    let services = ctx.services.clone();
    let bid_for_click = block_id.to_string();
    let toggle = div()
        .id(hashed_id(&format!(
            "drawer-toggle-{el_id_suffix}-{block_id}"
        )))
        .w(px(DRAWER_TOGGLE_WIDTH))
        .min_w(px(DRAWER_TOGGLE_WIDTH))
        .flex_shrink_0()
        .h_full()
        .cursor_col_resize()
        .hover(|s| s.bg(gpui::rgba(0x00000018)))
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            // Toggle relative to the *effective* state so the first tap on a
            // closed-by-default overlay drawer opens it (rather than writing a
            // redundant `false`).
            let current = services.drawer_open(&bid_for_click, mode);
            services.set_widget_open(&bid_for_click, !current);
        });
    TransparentTracker::new(
        drawer_toggle_id_for(block_id),
        "drawer_toggle",
        ctx.bounds_registry.clone(),
        toggle.into_any_element(),
    )
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let block_id = node.prop_str("block_id").unwrap_or_default();
    let mode = DrawerMode::from_str(
        &node
            .prop_str("mode")
            .unwrap_or_else(|| "shrink".to_string()),
    );
    let width = node.prop_f64("width").unwrap_or(300.0) as f32;
    let child = node.children.first().expect("drawer requires a child");

    let is_open = ctx.services.drawer_open(&block_id, mode);

    let rendered = super::render(child, ctx);
    let inner = div()
        .id(hashed_id(&block_id))
        .h_full()
        .overflow_y_scroll()
        .bg(tc(ctx, |t| t.sidebar))
        .w(px(width))
        .min_w(px(width))
        .px(px(ctx.style().sidebar_padding_x))
        .py(px(ctx.style().sidebar_padding_y))
        .text_sm()
        .border_r_1()
        .border_color(tc(ctx, |t| t.border))
        .child(rendered);

    let tracked_toggle = drawer_toggle_widget(&block_id, "drawer", mode, ctx);

    // Toggle first so the `overflow_hidden` clip in the closed
    // `Shrink` arm preserves it (collapsed width = toggle width). When
    // open, the toggle sits at the inner edge of the panel — the same
    // position a real user would reach for to close it.
    let panel_with_toggle = div()
        .h_full()
        .flex()
        .flex_row()
        .child(tracked_toggle)
        .child(inner);

    match mode {
        DrawerMode::Shrink => {
            // Shrink: takes layout space when open, collapses to 0 when closed.
            // The toggle stays at its known bounds even when closed (width 12px)
            // so a test click can re-open it.
            let target_width = if is_open {
                width + DRAWER_TOGGLE_WIDTH
            } else {
                DRAWER_TOGGLE_WIDTH
            };
            div()
                .h_full()
                .overflow_hidden()
                .flex_shrink_0()
                .w(px(target_width))
                .child(panel_with_toggle)
                .into_any_element()
        }
        DrawerMode::Overlay => {
            // Overlay: float above sibling content. The surrounding
            // `columns::render` is responsible for anchoring this panel
            // to the correct edge of the container (left or right) via
            // an absolute-positioned wrapper — we just return the panel
            // content (or the toggle alone when closed) so it remains
            // clickable.
            if is_open {
                panel_with_toggle.into_any_element()
            } else {
                // Render only the toggle when closed so the user can
                // re-open the drawer — same bounds-tracked element so
                // tests can click it.
                drawer_toggle_widget(&block_id, "drawer-overlay-closed", mode, ctx)
                    .into_any_element()
            }
        }
    }
}
