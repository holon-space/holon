use holon_frontend::ReactiveViewModel;

use super::prelude::*;

/// Inc 1 — inert passthrough: header (icon + title) then children, with NO
/// bounding yet. The bounded-footer split (cap + internal scroll) lands in
/// Inc 2 in `columns.rs`/`column.rs`; collapse (consulting the `expanded`
/// Mutable) lands in Inc 3. Until then this simply stacks its children so the
/// vocabulary parses, renders, and is observable via the `tag()` wrapper.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let title = node.prop_str("title").unwrap_or_default();
    let icon = node.prop_str("icon").unwrap_or_default();
    let children = &node.children;

    let mut header = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_color(tc(ctx, |t| t.foreground));
    if !icon.is_empty() {
        header = header.child(div().child(icon));
    }
    if !title.is_empty() {
        header = header.child(
            div()
                .flex_1()
                .font_weight(gpui::FontWeight::BOLD)
                .child(title),
        );
    }

    let mut container = div().w_full().flex().flex_col().child(header);
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}
