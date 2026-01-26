use super::prelude::*;
use holon_api::render_eval::{sort_key_column, sorted_rows};

pub fn build(ba: BA<'_>) -> TuiWidget {
    // In a TUI, screen layout columns just render vertically
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return TuiWidget::Empty,
    };

    let rows = sorted_rows(&ba.ctx.data_rows, sort_key_column(ba.args));

    if rows.is_empty() {
        let child_ctx = ba.ctx.with_row(Default::default());
        return (ba.interpret)(tmpl, &child_ctx);
    }

    let mut children = Vec::new();
    for row in &rows {
        let row_ctx = ba.ctx.with_row(row.clone());
        children.push((ba.interpret)(tmpl, &row_ctx));
    }

    TuiWidget::Column { children }
}
