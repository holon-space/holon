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
        ba.ctx
            .row()
            .iter()
            .map(|(key, value)| {
                AnyView::new(
                    text(format!("{key}: {}", value.to_display_string()))
                        .size(13.0)
                        .foreground(Color::srgb_hex("#808080")),
                )
            })
            .collect()
    };

    AnyView::new(vstack(views).spacing(4.0))
}
