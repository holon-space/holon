use super::prelude::*;
use holon_api::render_eval::{resolve_states, state_display};

holon_macros::widget_builder! {
    raw fn state_toggle(ba: BA<'_>) -> ViewModel {
        // state_toggle(col("task_state")): we need the field NAME, not the resolved value.
        let field = ba
            .args
            .get_positional_column_name(0)
            .or_else(|| ba.args.get_string("field"))
            .or_else(|| ba.args.get_positional_string(0))
            .unwrap_or("task_state")
            .to_string();

        let current = ba
            .ctx
            .row()
            .get(&field)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        let states = resolve_states(ba.args, ba.ctx.row()).join(",");
        let (label, _semantic) = state_display(&current);
        let label = label.to_string();

        ViewModel {
            kind: NodeKind::StateToggle { field, current, label, states },
            ..Default::default()
        }
    }
}
