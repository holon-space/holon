use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::{OperationIntent, ReactiveViewModel};

/// Handles ReactiveViewKind::Selectable — the "selectable" interaction wrapper.
///
/// Reads the action from the node's entity data (populated by the shadow builder
/// from the `action` named arg in the render DSL). The action name uses dot-notation:
/// `"navigation.focus"` → entity_name="navigation", op_name="focus".
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::reactive_view_model::ReactiveViewKind;
    let ReactiveViewKind::Selectable { child } = &node.kind else {
        unreachable!()
    };

    let child_el = super::render(child, ctx);

    let Some(Value::String(action_name)) = node.entity.get("__action_name") else {
        return child_el;
    };

    let (entity_name, op_name) = match action_name.split_once('.') {
        Some((e, o)) => (e.to_string(), o.to_string()),
        None => ("block".to_string(), action_name.clone()),
    };

    let mut params: HashMap<String, Value> = HashMap::new();
    for (k, v) in node.entity.iter() {
        if let Some(param_name) = k.strip_prefix("__action_") {
            if param_name != "name" {
                params.insert(param_name.to_string(), v.clone());
            }
        }
    }

    let row_id = node.row_id();
    let el_id = format!("selectable-{}", row_id.as_deref().unwrap_or("unknown"));
    let intent = OperationIntent::new(entity_name, op_name, params);
    let services = ctx.services.clone();

    let action_name_log = action_name.clone();
    let el_id_log = el_id.clone();
    let inner = div()
        .child(
            div()
                .id(hashed_id(&el_id))
                .cursor_pointer()
                .child(child_el)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    tracing::debug!("[selectable] CLICKED: el_id={}, action={}", el_id_log, action_name_log);
                    services.dispatch_intent(intent.clone());
                }),
        )
        .into_any_element();
    crate::geometry::tracked(el_id, inner, &ctx.bounds_registry, "selectable", row_id.as_deref(), true).into_any_element()
}
