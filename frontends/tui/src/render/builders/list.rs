use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let mut children = Vec::new();

    if let Some(tmpl) = template {
        if ba.ctx.data_rows.is_empty() {
            children.push((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &ba.ctx.data_rows {
                let row_ctx = ba.ctx.with_row(row.clone());
                children.push((ba.interpret)(tmpl, &row_ctx));
            }
        }
    } else {
        for (key, value) in ba.ctx.row() {
            children.push(TuiWidget::Text {
                content: format!("{key}: {}", value.to_display_string()),
                bold: false,
            });
        }
    }

    TuiWidget::Column { children }
}
