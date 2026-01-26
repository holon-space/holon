use std::collections::HashMap;

use super::prelude::*;
use blinc_core::Color;
use holon_api::Value;

use super::operation_helpers::{find_set_field_op, get_entity_name, get_row_id};
use holon_api::render_eval::{cycle_state, resolve_states, state_display};

fn semantic_to_color(name: &str, theme: &ThemeState) -> Color {
    match name {
        "muted" => theme.color(ColorToken::TextSecondary),
        "warning" => theme.color(ColorToken::Warning),
        "info" => theme.color(ColorToken::Info),
        "success" => theme.color(ColorToken::Success),
        _ => theme.color(ColorToken::TextPrimary),
    }
}

/// state_toggle(field_name, states:[...]) — cycle through states on click.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    let field = args
        .get_positional_string(0)
        .or_else(|| args.get_string("field"))
        .unwrap_or("task_state");

    let current = ctx
        .row()
        .get(field)
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let states = resolve_states(args, ctx.row());

    let (label, semantic) = state_display(&current);
    let color = semantic_to_color(semantic, &theme);

    let Some(op) = find_set_field_op(field, &ctx.operations) else {
        return div().child(text(label).size(13.0).color(color));
    };

    let row_id = get_row_id(ctx);
    let entity_name = get_entity_name(ctx).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let field_owned = field.to_string();
    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();

    div()
        .child(text(label).size(13.0).color(color))
        .on_click(move |_| {
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
