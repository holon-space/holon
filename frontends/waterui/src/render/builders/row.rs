use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let mut views: Vec<AnyView> = Vec::new();

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        views.push((ba.interpret)(template, ba.ctx));
    }

    for val in &ba.args.positional {
        if let Value::String(s) = val {
            views.push(AnyView::new(text(s.clone()).size(14.0)));
        }
    }

    AnyView::new(hstack(views).spacing(8.0))
}
