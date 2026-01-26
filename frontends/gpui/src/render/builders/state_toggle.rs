use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

use super::operation_helpers::{find_set_field_op, get_entity_name, get_row_id};
use holon_api::render_eval::{cycle_state, resolve_states, state_display};

fn semantic_color(ba: &BA<'_>, name: &str) -> Rgba {
    match name {
        "muted" => tc(ba, |t| t.text_secondary),
        "warning" => tc(ba, |t| t.warning),
        "info" => tc(ba, |t| t.primary_light),
        "success" => tc(ba, |t| t.success),
        _ => tc(ba, |t| t.text_primary),
    }
}

pub fn build(ba: BA<'_>) -> Div {
    let field = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("field"))
        .unwrap_or("task_state");

    let current = ba
        .ctx
        .row()
        .get(field)
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let states = resolve_states(ba.args, ba.ctx.row());
    let (label, semantic) = state_display(&current);
    let label = label.to_string();
    let color = semantic_color(&ba, semantic);

    let Some(op) = find_set_field_op(field, &ba.ctx.operations) else {
        return div().child(label).text_color(color);
    };

    let row_id = get_row_id(ba.ctx);
    let entity_name = get_entity_name(ba.ctx).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let field_owned = field.to_string();
    let session = ba.ctx.session.clone();
    let handle = ba.ctx.runtime_handle.clone();

    div()
        .child(label)
        .text_color(color)
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            let next = cycle_state(&current, &states);
            let Some(ref id) = row_id else { return };
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(id.clone()));
            params.insert("field".to_string(), Value::String(field_owned.clone()));
            params.insert("value".to_string(), Value::String(next));
            holon_frontend::operations::dispatch_operation(
                &handle,
                &session,
                entity_name.clone(),
                op_name.clone(),
                params,
            );
        })
}
