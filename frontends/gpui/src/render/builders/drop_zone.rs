use holon_frontend::ReactiveViewModel;
use holon_frontend::user_driver::DEFAULT_DROP_OP_NAME;
use holon_frontend::user_driver::build_drop_intent;

use super::prelude::*;
use crate::render::drag::DraggedBlock;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let target_id = node.row_id().map(|uri| uri.to_string());
    let target_entity = node.entity_name();
    let op_name = node
        .prop_str("op")
        .or_else(|| node.prop_str("op_name"))
        .unwrap_or_else(|| DEFAULT_DROP_OP_NAME.to_string());
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
                .unwrap_or_else(|| holon_api::EntityName::new("block"));

            // Drag state + row ids come from rendered rows. Resolve each once
            // through the row-id entry point; a value that names no entity has
            // no drop to dispatch.
            let (Some(source), Some(target)) = (
                holon_api::row_id_of_str(&dragged.block_id).entity(),
                holon_api::row_id_of_str(target).entity(),
            ) else {
                tracing::warn!(
                    "drop_zone: dragged {:?} onto {:?} names no entity",
                    dragged.block_id,
                    target
                );
                return;
            };
            if let Some(intent) = build_drop_intent(&source, &target, entity_name, &op_name) {
                services.dispatch_intent(intent);
            }
        })
}
