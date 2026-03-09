use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{find_ops_affecting, get_entity_name, get_row_id};

pub fn build(ba: BA<'_>) -> Div {
    let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
        (ba.interpret)(child_expr, ba.ctx)
    } else {
        div()
    };

    let fields: Vec<String> = match ba.args.named.get("fields") {
        Some(Value::String(s)) if s == "this" || s == "*" || s == "this.*" => ba
            .ctx
            .operations
            .iter()
            .flat_map(|ow| ow.descriptor.affected_fields.clone())
            .collect(),
        Some(val) => {
            let s = val.to_display_string();
            s.split(',').map(|f| f.trim().to_string()).collect()
        }
        None => return child,
    };

    let field_refs: Vec<&str> = fields.iter().map(|s| s.as_str()).collect();
    let ops = find_ops_affecting(&field_refs, &ba.ctx.operations);
    if ops.is_empty() {
        return child;
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

    let mut container = div().flex_col();
    container = container.child(child);

    let mut ops_row = div().flex().flex_row().gap_1();
    for (display_name, op_name, ent_name) in menu_items {
        let session = session.clone();
        let handle = handle.clone();
        let row_id = row_id.clone();
        ops_row = ops_row.child(
            div()
                .text_xs()
                .text_color(tc(&ba, |t| t.primary_light))
                .cursor_pointer()
                .child(display_name)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
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
