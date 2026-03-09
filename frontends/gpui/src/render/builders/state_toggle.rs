use std::collections::HashMap;

use super::prelude::*;
use holon_api::render_eval::{cycle_state, state_display};
use holon_api::Value;
use holon_frontend::ViewModel;

use super::operation_helpers::{entity_name_from_node, find_set_field_op, row_id_from_node};
use holon_frontend::operations::dispatch_operation;

fn semantic_color(ctx: &GpuiRenderContext, name: &str) -> Hsla {
    match name {
        "muted" => tc(ctx, |t| t.muted_foreground),
        "warning" => tc(ctx, |t| t.warning),
        "info" => tc(ctx, |t| t.accent),
        "success" => tc(ctx, |t| t.success),
        _ => tc(ctx, |t| t.foreground),
    }
}

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> Div {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::StateToggle {
        field,
        current,
        states,
        ..
    } = &node.kind
    else {
        unreachable!()
    };

    let (label, semantic) = state_display(current);
    let color = semantic_color(ctx, semantic);

    let Some(op) = find_set_field_op(field, &node.operations) else {
        return div().child(label.to_string()).text_color(color);
    };

    let row_id = row_id_from_node(node);
    let entity_name =
        entity_name_from_node(node).unwrap_or_else(|| op.entity_name.to_string());
    let op_name = op.name.clone();
    let field_owned = field.clone();
    let current_owned = current.clone();
    let states_vec: Vec<String> = states.split(',').map(|s| s.trim().to_string()).collect();
    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();
    let el_id = format!(
        "state-toggle-{}",
        row_id.as_deref().unwrap_or("unknown")
    );

    div().child(
        div()
            .id(ElementId::Name(el_id.into()))
            .cursor_pointer()
            .text_size(px(14.0))
            .child(label.to_string())
            .text_color(color)
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                let next = cycle_state(&current_owned, &states_vec);
                let Some(ref id) = row_id else { return };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.clone()));
                params.insert("field".to_string(), Value::String(field_owned.clone()));
                params.insert("value".to_string(), Value::String(next));
                dispatch_operation(
                    &handle,
                    &session,
                    entity_name.clone(),
                    op_name.clone(),
                    params,
                );
            }),
    )
}
