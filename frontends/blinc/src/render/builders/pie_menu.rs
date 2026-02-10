use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{find_ops_affecting, get_entity_name, get_row_id};
use crate::render::interpreter::interpret;

/// pie_menu(child_expr, fields:"field1,field2") — context menu wrapper.
///
/// Wraps a child element and attaches matching operations as a click menu.
/// When Blinc gets proper context menu support, this will use ContextMenuBuilder.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let child = if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        div()
    };

    // Resolve which fields to filter operations on
    let fields: Vec<String> = match args.named.get("fields") {
        Some(Value::String(s)) if s == "this" || s == "*" || s == "this.*" => {
            // All operations
            ctx.operations
                .iter()
                .flat_map(|ow| ow.descriptor.affected_fields.clone())
                .collect()
        }
        Some(val) => {
            let s = val.to_display_string();
            s.split(',').map(|f| f.trim().to_string()).collect()
        }
        None => return child,
    };

    let field_refs: Vec<&str> = fields.iter().map(|s| s.as_str()).collect();
    let ops = find_ops_affecting(&field_refs, &ctx.operations);
    if ops.is_empty() {
        return child;
    }

    let theme = ThemeState::get();
    let row_id = get_row_id(ctx);
    let entity_name = get_entity_name(ctx);
    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();

    // Build a simple operation list on click (context menu when overlay support exists)
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

    let mut container = div().flex_col();
    container = container.child(child);

    // Render operation buttons below child
    let mut ops_row = div().flex_row().gap(4.0);
    for (display_name, op_name, ent_name) in menu_items {
        let session = session.clone();
        let handle = handle.clone();
        let row_id = row_id.clone();
        ops_row = ops_row.child(
            div()
                .child(
                    text(display_name)
                        .size(11.0)
                        .color(theme.color(ColorToken::Accent)),
                )
                .on_click(move |_| {
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
                }),
        );
    }
    container.child(ops_row)
}
