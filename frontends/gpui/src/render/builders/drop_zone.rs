use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::{OperationIntent, ReactiveViewModel};

use crate::render::drag::DraggedBlock;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let target_id = node.row_id();
    let target_entity = node.entity_name().map(str::to_string);
    let services = ctx.services.clone();
    let drop_bg = tc(ctx, |t| t.accent);

    div()
        .h(px(4.0))
        .drag_over::<DraggedBlock>(move |style, _dragged, _, _| {
            style.h(px(8.0)).bg(drop_bg).rounded(px(2.0))
        })
        .on_drop(move |dragged: &DraggedBlock, _, _| {
            let Some(ref target) = target_id else {
                tracing::warn!("drop_zone: no target block id");
                return;
            };

            let entity_name = target_entity
                .clone()
                .unwrap_or_else(|| "block".to_string());

            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(dragged.block_id.clone()));
            params.insert("parent_id".to_string(), Value::String(target.clone()));

            services.dispatch_intent(OperationIntent::new(
                entity_name,
                "move_block".to_string(),
                params,
            ));
        })
}
