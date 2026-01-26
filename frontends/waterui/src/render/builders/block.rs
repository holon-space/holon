use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let indent = (ba.ctx.depth as f32) * 29.0;

    let child = if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else {
        AnyView::new(())
    };

    if indent > 0.0 {
        AnyView::new(hstack(vec![AnyView::new(spacer().width(indent)), child]))
    } else {
        child
    }
}
