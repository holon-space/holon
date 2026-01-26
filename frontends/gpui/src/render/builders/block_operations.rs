use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{find_ops_affecting, get_entity_name, get_row_id};

const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

pub fn build(ba: BA<'_>) -> Div {
    let ops = find_ops_affecting(BLOCK_FIELDS, &ba.ctx.operations);

    if ops.is_empty() {
        return div();
    }

    let row_id = get_row_id(ba.ctx);
    let entity_name = get_entity_name(ba.ctx);
    let session = ba.ctx.session.clone();
    let handle = ba.ctx.runtime_handle.clone();

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

    div()
        .text_xs()
        .text_color(tc(&ba, |t| t.text_secondary))
        .cursor_pointer()
        .child("[...]")
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            if let Some((_, op_name, ent_name)) = menu_items.first() {
                let Some(ref id) = row_id else { return };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.clone()));
                holon_frontend::operations::dispatch_operation(
                    &handle,
                    &session,
                    ent_name.clone(),
                    op_name.clone(),
                    params,
                );
            }
        })
}
