use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// Task-state pill. Left-click cycles the state and dispatches the same
/// `set_field(task_state, <next>)` write the GPUI widget and the headless
/// driver's `state_toggle_cycle_intent` produce (focus_path.rs) — op
/// lookup via `find_set_field_op` on the node's own operation wiring,
/// cycle math via the shared `cycle_state`.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
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

    let intent = (|| {
        let op = holon_frontend::operations::find_set_field_op(field, &node.operations)?;
        let states_vec: Vec<String> = states.split(',').map(|s| s.trim().to_string()).collect();
        let next = holon_api::render_eval::cycle_state(current, &states_vec);
        let entity_name = node.entity_name().unwrap_or_else(|| op.entity_name.clone());
        let row_id = node.row_id()?;
        Some(holon_frontend::OperationIntent::set_field(
            &entity_name,
            &op.name,
            &row_id,
            field,
            holon_api::Value::String(next),
        ))
    })();
    if intent.is_none() {
        // Disclose the degraded (display-only) pill: without op wiring or
        // a row id a click can't dispatch — same nodes GPUI renders inert.
        tracing::warn!(
            "[state_toggle] no set_field op wiring or row id for field '{field}' — rendering \
             display-only"
        );
    }

    rsx! {
        span {
            "data-role": "state-toggle",
            style: "cursor: pointer; font-size: 0.85em; color: #7fdf7f; padding: 1px 4px; border: 1px solid #3a3a3a; border-radius: 3px; user-select: none;",
            onclick: move |_| {
                let Some(intent) = intent.clone() else { return };
                crate::editor::dispatch_chain(vec![crate::editor::intent_to_wire(&intent)]);
            },
            "{display}"
        }
    }
}
