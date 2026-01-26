use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let child = if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else if let Some(first) = ba.args.positional_exprs.first() {
        (ba.interpret)(first, ba.ctx)
    } else {
        AnyView::new(text("[selectable]").size(12.0))
    };

    let action_expr = match ba.args.get_template("action") {
        Some(expr) => expr,
        None => return child,
    };

    if let Some(action) = holon_frontend::operations::parse_action_expr(action_expr, ba.ctx.row())
    {
        let session = ba.ctx.session.clone();
        let handle = ba.ctx.runtime_handle.clone();
        return AnyView::new(child.on_tap(move || {
            holon_frontend::operations::dispatch_operation(
                &handle,
                &session,
                action.entity_name.clone(),
                action.op_name.clone(),
                action.params.clone(),
            );
        }));
    }

    child
}
