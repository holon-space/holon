use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let title = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let mut views: Vec<AnyView> = vec![AnyView::new(text(title).size(18.0).bold())];

    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        if ba.ctx.data_rows.is_empty() {
            views.push((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &ba.ctx.data_rows {
                views.push((ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())));
            }
        }
    }

    AnyView::new(vstack(views).spacing(8.0).padding())
}
