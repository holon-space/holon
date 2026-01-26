use super::prelude::*;
use holon_frontend::operations::find_ops_affecting;
use holon_frontend::{OperationIntent, ReactiveViewModel};

const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let operations = node.prop_str("operations").unwrap_or_else(|| "".to_string());

    let ops = find_ops_affecting(BLOCK_FIELDS, &node.operations);

    if ops.is_empty() {
        if operations.is_empty() {
            return div();
        }
        return div()
            .text_xs()
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("[...]");
    }

    let row_id = node.row_id();
    let entity_name = node.entity_name();

    let first_op = ops[0];
    let intent_template =
        row_id.map(|id| OperationIntent::for_row(first_op, &id, entity_name.as_ref()));
    let el_id = format!(
        "block-ops-{}",
        intent_template
            .as_ref()
            .and_then(|i| i.params.get("id"))
            .map(|v| v.to_display_string())
            .unwrap_or_else(|| "x".into())
    );
    let services = ctx.services.clone();

    div().child(
        div()
            .id(hashed_id(&el_id))
            .cursor_pointer()
            .text_xs()
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("[...]")
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                if let Some(intent) = intent_template.clone() {
                    services.dispatch_intent(intent);
                }
            }),
    )
}
