use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let mut container = div().flex_col().gap_1();

    if let Some(tmpl) = template {
        if ba.ctx.data_rows.is_empty() {
            container = container.child((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &ba.ctx.data_rows {
                let row_ctx = ba.ctx.with_row(row.clone());
                container = container.child((ba.interpret)(tmpl, &row_ctx));
            }
        }
    } else {
        for (key, value) in ba.ctx.row() {
            container = container.child(format!("{key}: {}", value.to_display_string()));
        }
    }

    container
}
