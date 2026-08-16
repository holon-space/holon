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
    // A block with no task state has an empty label AND empty current — GPUI
    // renders nothing there. Emitting the pill anyway paints a stray bordered
    // box next to every plain block, so collapse it entirely.
    if display.trim().is_empty() {
        return rsx! {};
    }

    let intent = holon_frontend::operations::state_toggle_intent(
        field,
        current,
        states,
        &node.operations,
        node.entity_name().as_ref(),
        node.row_id().as_deref(),
    );
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
