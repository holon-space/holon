use std::collections::HashMap;

use super::prelude::*;
use holon_api::render_eval::{cycle_state, resolve_states, state_display};

pub fn build(ba: BA<'_>) -> Element {
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
    let color = semantic_to_css(semantic);
    let label = label.to_string();

    let Some(op) = holon_frontend::operations::find_set_field_op(field, &ba.ctx.operations) else {
        return rsx! { span { font_size: "13px", color: color, {label} } };
    };

    let row_id = holon_frontend::operations::get_row_id(ba.ctx);
    let entity_name =
        holon_frontend::operations::get_entity_name(ba.ctx).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let field_owned = field.to_string();
    let session = ba.ctx.session().clone();

    rsx! {
        span {
            font_size: "13px",
            color: color,
            cursor: "pointer",
            onclick: move |_| {
                let next = cycle_state(&current, &states);
                let Some(ref id) = row_id else { return };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.clone()));
                params.insert("field".to_string(), Value::String(field_owned.clone()));
                params.insert("value".to_string(), Value::String(next));
                crate::operations::dispatch_operation(
                    &session,
                    entity_name.clone(),
                    op_name.clone(),
                    params,
                );
            },
            {label}
        }
    }
}

fn semantic_to_css(name: &str) -> &str {
    match name {
        "muted" => "var(--text-muted)",
        "warning" => "var(--warning)",
        "info" => "var(--info)",
        "success" => "var(--success)",
        "primary" => "var(--text-primary)",
        _ => "inherit",
    }
}
