use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    if let Some(child_expr) = ba.args.positional_exprs.first() {
        (ba.interpret)(child_expr, ba.ctx)
    } else if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else {
        rsx! {}
    }
}
