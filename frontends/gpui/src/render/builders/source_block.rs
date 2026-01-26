use super::prelude::*;
use holon_frontend::{OperationIntent, ReactiveViewModel};

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let language = node.prop_str("language").unwrap_or_else(|| "text".to_string());
    let content = node.prop_str("content").unwrap_or_else(|| "".to_string());
    let name = node.prop_str("name").unwrap_or_else(|| "".to_string());

    let mut container = div().flex_col().gap_1();

    let mut header = div().flex().flex_row().gap_2();
    header = header.child(
        div()
            .text_xs()
            .text_color(tc(ctx, |t| t.accent))
            .child(language),
    );
    if !name.is_empty() {
        header = header.child(
            div()
                .text_xs()
                .text_color(tc(ctx, |t| t.muted_foreground))
                .child(name),
        );
    }

    let exec_op = node
        .operations
        .iter()
        .find(|ow| ow.descriptor.name == "execute_source_block");
    if let Some(exec_op) = exec_op {
        let row_id = node.row_id();
        let entity_name = node.entity_name();
        let intent_template = row_id.map(|id| {
            OperationIntent::for_row(&exec_op.descriptor, &id, entity_name.as_ref())
        });
        let el_id = format!(
            "src-run-{}",
            intent_template
                .as_ref()
                .and_then(|i| i.params.get("id"))
                .map(|v| v.to_display_string())
                .unwrap_or_else(|| "x".into())
        );
        let services = ctx.services.clone();

        header = header.child(
            div()
                .id(hashed_id(&el_id))
                .cursor_pointer()
                .text_xs()
                .text_color(tc(ctx, |t| t.success))
                .child("[run]")
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    if let Some(intent) = intent_template.clone() {
                        services.dispatch_intent(intent);
                    }
                }),
        );
    }
    container = container.child(header);

    container.child(
        div()
            .rounded(px(6.0))
            .bg(tc(ctx, |t| t.secondary))
            .overflow_hidden()
            .px(px(12.0))
            .py(px(10.0))
            .text_xs()
            .line_height(px(18.0))
            .child(content),
    )
}
