use holon_frontend::ReactiveViewModel;

use super::prelude::*;

const DEFAULT_MAX_HEIGHT_FRACTION: f32 = 0.4;

/// Whether the accordion is currently expanded (live state on the node's
/// `expanded` Mutable, seeded from the `collapsed` prop — R6: never the
/// `ctx.local` title-keyed cache). Absent handle ⇒ treated as expanded.
fn is_expanded(node: &ReactiveViewModel) -> bool {
    node.expanded.as_ref().map(|m| m.get()).unwrap_or(true)
}

/// Header row: chevron (reflects expanded state) + optional icon + title.
///
/// When `collapsible` (default), the header is a click target that flips the
/// node's `expanded` Mutable and refreshes the window (the established
/// expand_toggle pattern — R6: the live state is the node Mutable, never the
/// `ctx.local` title-keyed cache). The next render reads `is_expanded` and
/// renders or drops the body.
fn header_row(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let title = node.prop_str("title").unwrap_or_default();
    let icon = node.prop_str("icon").unwrap_or_default();
    let glyph = if is_expanded(node) { "▾" } else { "▸" };

    let mut header = div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_color(tc(ctx, |t| t.foreground))
        .child(div().text_sm().child(glyph));
    if !icon.is_empty() {
        header = header.child(div().child(icon));
    }
    if !title.is_empty() {
        header = header.child(
            div()
                .flex_1()
                .font_weight(gpui::FontWeight::BOLD)
                .child(title.clone()),
        );
    }

    // Collapsible header is a click target: flip the node's `expanded` Mutable
    // and refresh the window (the established expand_toggle pattern). The next
    // render reads `is_expanded` and renders or drops the body.
    let collapsible = node.prop_bool("collapsible").unwrap_or(true);
    match (collapsible, node.expanded.clone()) {
        (true, Some(expanded)) => header
            .id(hashed_id(&format!("accordion-toggle:{title}")))
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
                expanded.set(!expanded.get());
                window.refresh();
            })
            .into_any_element(),
        _ => header.into_any_element(),
    }
}

/// The bounded-footer accordion element (plan §4). Called by the flow-panel
/// split in `columns.rs`/`column.rs` for a correctly-placed accordion (direct
/// child of a flow column). Its `max_h(relative(f))` resolves against the
/// split's definite-height absolute wrapper (verified by the Inc 0 spike), so
/// with overflowing content the region caps at `f × panel_height` and its body
/// is its own scroll viewport; with little content it shrinks to content.
pub(crate) fn render_bounded(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let fraction = node
        .prop_f64("max_height_fraction")
        .unwrap_or(DEFAULT_MAX_HEIGHT_FRACTION as f64) as f32;

    let mut region = div()
        .flex_shrink_0()
        .w_full()
        .flex()
        .flex_col()
        .max_h(gpui::relative(fraction))
        .child(header_row(node, ctx));

    if is_expanded(node) {
        let title = node.prop_str("title").unwrap_or_default();
        let body_id = hashed_id(&format!("accordion-body:{title}"));
        // Body is its own `min_h_0 + overflow_y_scroll` viewport (the April-2026
        // cascade lesson: without `min_h_0` the content height becomes the min
        // and the internal scroll freezes). Content is rendered EAGERLY at
        // content height (R4: `live_query`'s greedy `relative(1.0)` ReactiveShell
        // would otherwise always claim the full cap and defeat shrink-to-content;
        // backlink lists are small, so eager is appropriate here permanently).
        region = region.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .id(body_id)
                .overflow_y_scroll()
                .child(super::column::render_children_content_height(
                    &node.children,
                    ctx,
                )),
        );
    }
    region
}

/// Generic dispatch path — reached ONLY when an accordion is rendered somewhere
/// other than the flow-panel split (it is not a direct child of a flow column).
/// The split intercepts every correctly-placed accordion before this, so
/// reaching here means the accordion is misplaced: render the standard error
/// widget (the render-time half of the §3 fail-loud placement guard) rather
/// than a silently-unbounded region.
pub fn render(_node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| t.danger))
        .text_sm()
        .child("accordion must be a direct child of a main-panel column")
}
