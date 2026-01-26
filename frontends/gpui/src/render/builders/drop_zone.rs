use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;

use super::operation_helpers::{entity_name_from_node, row_id_from_node};
use crate::render::drag::DraggedBlock;
use holon_frontend::operations::dispatch_operation;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> Div {
    let target_id = row_id_from_node(node);
    let target_entity = entity_name_from_node(node);
    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();
    let drop_bg = tc(ctx, |t| t.accent);

    div()
        .h(px(4.0))
        .drag_over::<DraggedBlock>(move |style, _dragged, _, _| {
            style.h(px(8.0)).bg(drop_bg).rounded(px(2.0))
        })
        .on_drop(move |dragged: &DraggedBlock, _, _| {
            let move_op = dragged
                .operations
                .iter()
                .find(|ow| ow.descriptor.name == "move_block");

            let Some(op) = move_op else {
                tracing::warn!("drop_zone: no move_block operation on dragged block");
                return;
            };

            let Some(ref target) = target_id else {
                tracing::warn!("drop_zone: no target block id");
                return;
            };

            let entity_name = target_entity
                .clone()
                .unwrap_or_else(|| op.descriptor.entity_name.to_string());

            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(dragged.block_id.clone()));
            params.insert("parent_id".to_string(), Value::String(target.clone()));

            dispatch_operation(
                &handle,
                &session,
                entity_name,
                "move_block".to_string(),
                params,
            );
        })
}
