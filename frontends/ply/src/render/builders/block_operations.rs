use std::collections::HashMap;

use super::operation_helpers::{find_ops_affecting, get_entity_name, get_row_id};
use super::prelude::*;
use holon_api::Value;
use holon_frontend::operations::OperationIntent;

const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let ops = find_ops_affecting(BLOCK_FIELDS, &ctx.operations);
    if ops.is_empty() {
        return empty_widget();
    }

    let row_id = get_row_id(ctx);
    let entity_name = get_entity_name(ctx);
    let services = ctx.services.clone();

    let menu_items: Vec<(String, String, String)> = ops
        .iter()
        .map(|op| {
            (
                op.display_name.clone(),
                op.name.clone(),
                entity_name
                    .clone()
                    .unwrap_or_else(|| op.entity_name.to_string()),
            )
        })
        .collect();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        let services = services.clone();
        let row_id = row_id.clone();
        let menu_items = menu_items.clone();
        ui.element()
            .on_press(move |_id, _pointer| {
                if let Some((_, op_name, ent_name)) = menu_items.first() {
                    let Some(ref id) = row_id else { return };
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(id.clone()));
                    services.dispatch_intent(OperationIntent {
                        entity_name: holon_api::EntityName::new(ent_name.as_str()),
                        op_name: op_name.clone(),
                        params,
                    });
                }
            })
            .children(|ui| {
                ui.text("[...]", |t| t.font_size(11).color(0x888888u32));
            });
    })
}
