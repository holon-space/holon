use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;

use holon_frontend::operations::dispatch_operation;

/// Handles NodeKind::Selectable — the "selectable" interaction wrapper.
///
/// Reads the action from the node's entity data (populated by the shadow builder
/// from the `action` named arg in the render DSL). The action name uses dot-notation:
/// `"navigation.focus"` → entity_name="navigation", op_name="focus".
pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::Selectable { child } = &node.kind else {
        unreachable!()
    };

    let child_el = super::render(child, ctx);

    let Some(Value::String(action_name)) = node.entity.get("__action_name") else {
        return child_el;
    };

    // Parse dot-notation: "navigation.focus" → ("navigation", "focus")
    let (entity_name, op_name) = match action_name.split_once('.') {
        Some((e, o)) => (e.to_string(), o.to_string()),
        None => ("block".to_string(), action_name.clone()),
    };

    // Collect __action_* params (excluding __action_name)
    let mut params: HashMap<String, Value> = HashMap::new();
    for (k, v) in &node.entity {
        if let Some(param_name) = k.strip_prefix("__action_") {
            if param_name != "name" {
                params.insert(param_name.to_string(), v.clone());
            }
        }
    }

    let row_id = node
        .entity
        .get("id")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    let el_id = format!("selectable-{}", row_id.as_deref().unwrap_or("unknown"));

    let session = ctx.session().clone();
    let handle = ctx.runtime_handle().clone();

    div()
        .child(
            div()
                .id(ElementId::Name(el_id.into()))
                .cursor_pointer()
                .child(child_el)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    dispatch_operation(
                        &handle,
                        &session,
                        entity_name.clone(),
                        op_name.clone(),
                        params.clone(),
                    );
                }),
        )
        .into_any_element()
}
