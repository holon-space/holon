use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let child = if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else if let Some(first) = ba.args.positional_exprs.first() {
        (ba.interpret)(first, ba.ctx)
    } else {
        rsx! { span { font_size: "12px", "[selectable]" } }
    };

    let action_expr = match ba.args.get_template("action") {
        Some(expr) => expr,
        None => return child,
    };

    if let Some(action) = holon_frontend::operations::parse_action_expr(action_expr, ba.ctx.row())
    {
        let session = ba.ctx.session().clone();
        return rsx! {
            div {
                cursor: "pointer",
                onclick: move |_| {
                    crate::operations::dispatch_operation(
                        &session,
                        action.entity_name.clone(),
                        action.op_name.clone(),
                        action.params.clone(),
                    );
                },
                {child}
            }
        };
    }

    child
}
