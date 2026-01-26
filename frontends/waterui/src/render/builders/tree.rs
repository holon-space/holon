use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let views: Vec<AnyView> = if let Some(tmpl) = template {
        if ba.ctx.data_rows.is_empty() {
            vec![(ba.interpret)(tmpl, ba.ctx)]
        } else {
            ba.ctx
                .data_rows
                .iter()
                .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
                .collect()
        }
    } else {
        vec![AnyView::new(text("[tree: no template]").size(12.0))]
    };

    AnyView::new(vstack(views).spacing(4.0))
}
