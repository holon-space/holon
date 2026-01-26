use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else {
        TuiWidget::Empty
    }
}
