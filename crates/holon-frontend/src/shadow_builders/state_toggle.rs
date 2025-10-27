use super::prelude::*;
use holon_api::render_eval::{resolve_states, state_display};

pub fn build(ba: BA<'_>) -> DisplayNode {
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
    let (label, _semantic) = state_display(&current);
    let label = label.to_string();

    DisplayNode::element(
        "state_toggle",
        [
            ("field".into(), Value::String(field.to_string())),
            ("current".into(), Value::String(current)),
            ("label".into(), Value::String(label)),
            (
                "states".into(),
                Value::String(states.join(",")),
            ),
        ]
        .into(),
        vec![],
    )
}
