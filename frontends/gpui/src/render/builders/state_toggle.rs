use super::prelude::*;
use holon_api::render_eval::{cycle_state, state_display, state_icon};
use holon_api::Value;
use holon_frontend::operations::find_set_field_op;
use holon_frontend::{OperationIntent, ReactiveViewModel};

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
    let field = node.prop_str("field").unwrap_or_else(|| "task_state".to_string());
    let current = node.prop_str("current").unwrap_or_else(|| "".to_string());
    let states = node.prop_str("states").unwrap_or_else(|| "".to_string());

    let (_label, semantic) = state_display(&current);
    let color = semantic_color(ctx, semantic);
    let icon = state_icon(&current);

    let Some(op) = find_set_field_op(&field, &node.operations) else {
        return div()
            .flex_shrink_0()
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

    let row_id = node.row_id();
    let entity_name = node.entity_name().unwrap_or_else(|| op.entity_name.clone());
    let op_name = op.name.clone();
    let field_owned = field.clone();
    let current_owned = current.clone();
    let states_vec: Vec<String> = states.split(',').map(|s| s.trim().to_string()).collect();
    let el_id = format!(
        "state-toggle-{}",
        row_id.as_deref().unwrap_or("unknown")
    );
    let services = ctx.services.clone();

    // The outer div is sized exactly like icon::render (20×16) so it
    // occupies the same space as the orgmode bullet in non-task blocks.
    div()
        .flex_shrink_0()
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
                    let next = cycle_state(&current_owned, &states_vec);
                    let Some(ref id) = row_id else { return };
                    let intent = OperationIntent::set_field(
                        &entity_name,
                        &op_name,
                        id,
                        &field_owned,
                        Value::String(next),
                    );
                    services.dispatch_intent(intent);
                }),
        )
}
