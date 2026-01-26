use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{find_ops_affecting, get_entity_name, get_row_id};

const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

/// block_operations() — "..." menu button showing structural operations.
pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    let ops = find_ops_affecting(BLOCK_FIELDS, &ctx.operations);

    if ops.is_empty() {
        return div();
    }

    // Build menu items
    let row_id = get_row_id(ctx);
    let entity_name = get_entity_name(ctx);
    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();

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
        .child(
            text("[...]".to_string())
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        )
        .on_click(move |_| {
            // For now, dispatch the first operation (context menu requires Blinc overlay support)
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
