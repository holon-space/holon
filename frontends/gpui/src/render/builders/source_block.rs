use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;

use super::operation_helpers::{entity_name_from_node, row_id_from_node};
use holon_frontend::operations::dispatch_operation;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> Div {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::SourceBlock {
        language,
        content,
        name,
        ..
    } = &node.kind
    else {
        unreachable!()
    };

    let mut container = div().flex_col().gap_1();

    let mut header = div().flex().flex_row().gap_2();
    header = header.child(
        div()
            .text_xs()
            .text_color(tc(ctx, |t| t.accent))
            .child(language.clone()),
    );
    if !name.is_empty() {
        header = header.child(
            div()
                .text_xs()
                .text_color(tc(ctx, |t| t.muted_foreground))
                .child(name.clone()),
        );
    }

    // Execute button if execute_source_block operation is available
    let exec_op = node
        .operations
        .iter()
        .find(|ow| ow.descriptor.name == "execute_source_block");
    if let Some(exec_op) = exec_op {
        let row_id = row_id_from_node(node);
        let entity_name = entity_name_from_node(node)
            .unwrap_or_else(|| exec_op.descriptor.entity_name.to_string());
        let op_name = exec_op.descriptor.name.clone();
        let session = ctx.session().clone();
        let handle = ctx.runtime_handle().clone();
        let el_id = format!("src-run-{}", row_id.as_deref().unwrap_or("x"));

        header = header.child(
            div()
                .id(ElementId::Name(el_id.into()))
                .cursor_pointer()
                .text_xs()
                .text_color(tc(ctx, |t| t.success))
                .child("[run]")
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    let Some(ref id) = row_id else { return };
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(id.clone()));
                    dispatch_operation(
                        &handle,
                        &session,
                        entity_name.clone(),
                        op_name.clone(),
                        params,
                    );
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
            .child(content.clone()),
    )
}
