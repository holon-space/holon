use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;

use super::operation_helpers::{entity_name_from_node, find_ops_affecting, row_id_from_node};
use holon_frontend::operations::dispatch_operation;

const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> Div {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::BlockOperations { operations } = &node.kind else {
        unreachable!()
    };

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

    let row_id = row_id_from_node(node);
    let entity_name = entity_name_from_node(node);
    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();

    // For now, dispatch the first operation on click (context menu when overlay support exists)
    let first_op = &ops[0];
    let ent_name = entity_name
        .unwrap_or_else(|| first_op.entity_name.to_string());
    let op_name = first_op.name.clone();
    let el_id = format!("block-ops-{}", row_id.as_deref().unwrap_or("x"));

    div().child(
        div()
            .id(ElementId::Name(el_id.into()))
            .cursor_pointer()
            .text_xs()
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("[...]")
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                let Some(ref id) = row_id else { return };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.clone()));
                dispatch_operation(
                    &handle,
                    &session,
                    ent_name.clone(),
                    op_name.clone(),
                    params,
                );
            }),
    )
}
