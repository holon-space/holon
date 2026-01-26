use std::sync::Arc;

use holon_api::Value;
use holon_api::render_eval::cycle_state;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::operations::find_set_field_op;
use holon_frontend::view_model::ViewKind;

use super::dispatch::dispatch_intent;
use super::prelude::*;

const STYLE: &str = "cursor: pointer; font-size: 0.85em; color: #7fdf7f; padding: 1px 4px; \
                     border: 1px solid #3a3a3a; border-radius: 3px;";

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::StateToggle { .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { StateToggleNode { node: node.clone() } }
}

/// A task-state badge that cycles its state (TODO → DOING → DONE → …) on click
/// by dispatching `set_field`. Mirrors gpui `state_toggle.rs`; falls back to a
/// static badge when no `set_field` op is wired onto the node.
#[component]
fn StateToggleNode(node: ViewModel) -> Element {
    // Hooks first (unconditional) so the early returns below can't reorder them.
    let session: Arc<FrontendSession> = use_context();
    let rt: tokio::runtime::Handle = use_context();

    let ViewKind::StateToggle {
        field,
        current,
        label,
        states,
    } = &node.kind
    else {
        return rsx! {};
    };
    let display = if label.is_empty() {
        current.clone()
    } else {
        label.clone()
    };

    // No set_field op or no row id → static badge (no dispatch, no fake action).
    let (Some(op), Some(row_id)) = (find_set_field_op(field, &node.operations), node.row_id())
    else {
        return rsx! { span { style: STYLE, "{display}" } };
    };

    let entity_name = node.entity_name().unwrap_or_else(|| op.entity_name.clone());
    let op_name = op.name.clone();
    let field = field.clone();
    let current = current.clone();
    let states_vec: Vec<String> = states.split(',').map(|s| s.trim().to_string()).collect();

    rsx! {
        span {
            style: STYLE,
            onmousedown: move |evt| {
                evt.stop_propagation();
                let next = cycle_state(&current, &states_vec);
                let intent = OperationIntent::set_field(
                    &entity_name,
                    &op_name,
                    &row_id,
                    &field,
                    Value::String(next),
                );
                dispatch_intent(&rt, &session, intent);
            },
            "{display}"
        }
    }
}
