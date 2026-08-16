use holon_api::render_eval::state_display;
use holon_api::render_eval::state_icon;
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::state_toggle_intent;

use super::prelude::*;

fn semantic_color(ctx: &GpuiRenderContext, name: &str) -> Hsla {
    match name {
        "muted" => tc(ctx, |t| t.muted_foreground),
        "warning" => tc(ctx, |t| t.warning),
        "info" => tc(ctx, |t| t.accent),
        "success" => tc(ctx, |t| t.success),
        _ => tc(ctx, |t| t.foreground),
    }
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let field = node
        .prop_str("field")
        .unwrap_or_else(|| "task_state".to_string());
    let current = node.prop_str("current").unwrap_or_default();
    let states = node.prop_str("states").unwrap_or_default();
    let mt = node.prop_f64("mt").unwrap_or(0.0) as f32;

    // Non-task block: no task_state, so there is no checkbox to show. Collapse
    // the placeholder to zero width instead of reserving a full icon box —
    // otherwise every ordinary block carries a wide empty gutter before its
    // text (dogfood PHASE 3 bug 8).
    if current.is_empty() {
        return div().flex_shrink_0();
    }

    let (_label, semantic) = state_display(&current);
    let color = semantic_color(ctx, semantic);
    let icon = state_icon(&current);

    let row_id = node.row_id();
    let intent = state_toggle_intent(
        &field,
        &current,
        &states,
        &node.operations,
        node.entity_name().as_ref(),
        row_id.as_deref(),
    );
    let Some(intent) = intent else {
        // Disclose the degraded (display-only) glyph: without op wiring or a
        // row id a click cannot dispatch.
        tracing::warn!(
            block_id = row_id.as_deref().unwrap_or("<none>"),
            "state_toggle: no set_field op wiring or row id for field '{field}' — rendering \
             display-only"
        );
        return div()
            .flex_shrink_0()
            .mt(px(mt))
            .w(px(ctx.style().icon_size + ctx.style().icon_box_padding))
            .h(px(ctx.style().icon_size))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(ctx.style().icon_size))
            .line_height(px(ctx.style().icon_size))
            .text_color(color)
            .child(icon);
    };

    let el_id = format!("state-toggle-{}", row_id.as_deref().unwrap_or("unknown"));
    let services = ctx.services.clone();

    // The outer div is sized exactly like icon::render (20×16) so the task
    // checkbox lines up with the orgmode bullet's box.
    div()
        .flex_shrink_0()
        .mt(px(mt))
        .w(px(ctx.style().icon_size + ctx.style().icon_box_padding))
        .h(px(ctx.style().icon_size))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .id(hashed_id(&el_id))
                .cursor_pointer()
                .text_size(px(ctx.style().icon_size))
                .line_height(px(ctx.style().icon_size))
                .text_color(color)
                .child(icon)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    services.dispatch_intent(intent.clone());
                }),
        )
}
