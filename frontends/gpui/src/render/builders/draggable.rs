use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    tracing::debug!("draggable: drag not yet supported in GPUI frontend, rendering child only");

    if let Some(child_expr) = ba.args.positional_exprs.first() {
        (ba.interpret)(child_expr, ba.ctx)
    } else {
        div()
    }
}
