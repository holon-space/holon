use std::collections::HashMap;

use super::operation_helpers::{find_set_field_op, get_entity_name, get_row_id};
use super::prelude::*;
use holon_api::render_eval::{cycle_state, resolve_states, state_display};
use holon_api::Value;

fn semantic_to_rgb(name: &str) -> u32 {
    match name {
        "muted" => 0x808080,
        "warning" => 0xFFA000,
        "info" => 0x42A5F5,
        "success" => 0x4CAF50,
        _ => 0xE0E0E0,
    }
}

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
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
    let label = label.to_string();
    let color = semantic_to_rgb(semantic);

    let Some(op) = find_set_field_op(field, &ctx.operations) else {
        return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text(&label, |t| t.font_size(14).color(color));
        });
    };

    let row_id = get_row_id(ctx);
    let entity_name = get_entity_name(ctx).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let field_owned = field.to_string();
    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        let session = session.clone();
        let handle = handle.clone();
        let entity_name = entity_name.clone();
        let op_name = op_name.clone();
        let current = current.clone();
        let states = states.clone();
        let row_id = row_id.clone();
        let field_owned = field_owned.clone();
        ui.element()
            .on_press(move |_id, _pointer| {
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
            .children(|ui| {
                ui.text(&label, |t| t.font_size(14).color(color));
            });
    })
}
