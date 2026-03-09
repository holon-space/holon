use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let title = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let mut container = div()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(px(8.0))
        .bg(tc(&ba, |t| t.sidebar_background));

    container = container.child(
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title),
    );

    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        if ba.ctx.data_rows.is_empty() {
            container = container.child((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &ba.ctx.data_rows {
                let row_ctx = ba.ctx.with_row(row.clone());
                container = container.child((ba.interpret)(tmpl, &row_ctx));
            }
        }
    } else {
        for expr in &ba.args.positional_exprs {
            container = container.child((ba.interpret)(expr, ba.ctx));
        }
    }

    container
}
