use holon_frontend::drawer_toggle_id_for;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::DrawerMode;

use super::prelude::*;
use crate::geometry::TransparentTracker;

/// Width of the clickable drawer-toggle handle. Stays visible in the
/// closed state so a user (or the layout PBT's `ToggleDrawer`
/// transition) can re-open the drawer. Shared between `drawer::render`
/// and the columns special-case for first/last shrink drawers.
pub(super) const DRAWER_TOGGLE_WIDTH: f32 = 12.0;

/// Clamp range for a user-dragged sidebar width. Below the minimum the
/// panel content is unusable; above the maximum it swallows the main area.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 160.0;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 480.0;

/// Cursor travel (px) that separates a drag from a click. Below this a
/// press-release on the handle is treated as a collapse/expand toggle;
/// above it, it's a resize and the toggle is suppressed.
const DRAG_THRESHOLD: f32 = 3.0;

/// Which window edge the sidebar is pinned to. Determines how a live cursor
/// x-position maps to a panel width: a left sidebar's left edge is fixed at
/// window x=0 (width == cursor x), a right sidebar's right edge is fixed at
/// the viewport width (width == viewport_width − cursor x).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeAnchor {
    Left,
    Right,
}

impl ResizeAnchor {
    pub(crate) fn width_at(self, cursor_x: f32, viewport_width: f32) -> f32 {
        match self {
            ResizeAnchor::Left => cursor_x,
            ResizeAnchor::Right => viewport_width - cursor_x,
        }
    }
}

/// In-progress sidebar drag. Lives as a GPUI global so the handle (which
/// begins the drag) and the root view's full-window capture overlay (which
/// tracks the cursor and finalizes) can share it without threading state
/// through the stateless render builders.
#[derive(Clone, Debug)]
pub(crate) struct SidebarResize {
    pub block_id: String,
    pub mode: DrawerMode,
    pub anchor: ResizeAnchor,
    /// Cursor x at mouse-down, used to measure travel against `DRAG_THRESHOLD`.
    pub start_x: f32,
    /// Set once the cursor has travelled past the threshold — the press is a
    /// resize, not a toggle.
    pub moved: bool,
}

#[derive(Default)]
pub(crate) struct SidebarResizeState {
    pub active: Option<SidebarResize>,
}

impl gpui::Global for SidebarResizeState {}

/// Effective render width for a drawer: a user-dragged width (persisted per
/// block in `WidgetState`) overrides the projected default, clamped to the
/// sane range. `prop_width` is the projection's default (typically 300).
pub(super) fn effective_drawer_width(
    services: &dyn BuilderServices,
    block_id: &str,
    prop_width: f32,
) -> f32 {
    services
        .widget_state_explicit(block_id)
        .and_then(|s| s.width)
        .map(|w| w.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH))
        .unwrap_or(prop_width)
}

/// Complete an in-progress sidebar drag. A resize (cursor moved past the
/// threshold) persists the final width to disk; a plain click toggles the
/// drawer's collapsed state. Called from both the handle's mouse-up (plain
/// clicks release over the 12px handle) and the capture overlay's mouse-up
/// (drags release wherever the cursor ended up).
pub(crate) fn finalize_sidebar_resize(
    services: &dyn BuilderServices,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let done = cx.default_global::<SidebarResizeState>().active.take();
    if let Some(active) = done {
        if active.moved {
            if let Some(width) = services
                .widget_state_explicit(&active.block_id)
                .and_then(|s| s.width)
            {
                services.set_widget_width(&active.block_id, width, true);
            }
        } else {
            let current = services.drawer_open(&active.block_id, active.mode);
            services.set_widget_open(&active.block_id, !current);
        }
        window.refresh();
    }
}

/// Advance a live sidebar drag by one cursor sample. Once the cursor has
/// travelled past the click threshold the drag is marked `moved` and each
/// sample applies the new width in-memory (no disk write — that happens once
/// on release). Returns `true` when the width changed and the window needs a
/// repaint. A no-op (returns `false`) when there is no active drag or the
/// cursor hasn't yet moved far enough to count as a resize.
pub(crate) fn drag_sidebar_to(
    services: &dyn BuilderServices,
    cursor_x: f32,
    viewport_width: f32,
    cx: &mut gpui::App,
) -> bool {
    let (block_id, anchor, moved) = {
        let st = cx.default_global::<SidebarResizeState>();
        let Some(active) = st.active.as_mut() else {
            return false;
        };
        if (cursor_x - active.start_x).abs() > DRAG_THRESHOLD {
            active.moved = true;
        }
        (active.block_id.clone(), active.anchor, active.moved)
    };
    if !moved {
        return false;
    }
    let width = anchor
        .width_at(cursor_x, viewport_width)
        .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
    services.set_widget_width(&block_id, width, false);
    true
}

/// The bounds-tracked clickable toggle widget for `block_id`'s drawer.
/// Both renderers use this — the variant-specific framing (where the
/// toggle sits relative to the panel, how the overlay anchors it) lives
/// at the call site; the toggle widget itself is shared.
///
/// The handle doubles as the sidebar's resize grip: a press begins a drag
/// (recorded in [`SidebarResizeState`]); a release without travel toggles the
/// drawer, a release after travel commits the dragged width. The root view's
/// capture overlay drives the live resize while the button is held.
///
/// `anchor` tells the drag which window edge the sidebar is pinned to.
///
/// `el_id_suffix` differentiates the GPUI element id between call
/// sites (a single drawer can be rendered via either `drawer::render`
/// or the columns shrink fast-path, but never both simultaneously, so
/// the suffix only needs to be unique within one render pass).
pub(super) fn drawer_toggle_widget(
    block_id: &str,
    el_id_suffix: &str,
    mode: DrawerMode,
    anchor: ResizeAnchor,
    ctx: &GpuiRenderContext,
) -> impl IntoElement {
    let services_up = ctx.services.clone();
    let bid_down = block_id.to_string();
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
        .on_mouse_down(gpui::MouseButton::Left, move |ev, window, cx| {
            cx.default_global::<SidebarResizeState>().active = Some(SidebarResize {
                block_id: bid_down.clone(),
                mode,
                anchor,
                start_x: f32::from(ev.position.x),
                moved: false,
            });
            window.refresh();
        })
        .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
            finalize_sidebar_resize(&*services_up, window, cx);
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
    let prop_width = node.prop_f64("width").unwrap_or(300.0) as f32;
    let width = effective_drawer_width(&*ctx.services, &block_id, prop_width);
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

    let tracked_toggle = drawer_toggle_widget(&block_id, "drawer", mode, ResizeAnchor::Left, ctx);

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
                drawer_toggle_widget(
                    &block_id,
                    "drawer-overlay-closed",
                    mode,
                    ResizeAnchor::Left,
                    ctx,
                )
                .into_any_element()
            }
        }
    }
}
