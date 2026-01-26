use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    if ba.ctx.data_rows.is_empty() {
        return AnyView::new(
            text("[empty]")
                .size(12.0)
                .foreground(Color::srgb_hex("#808080")),
        );
    }

    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        let views: Vec<AnyView> = ba
            .ctx
            .data_rows
            .iter()
            .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
            .collect();
        return AnyView::new(vstack(views).spacing(2.0));
    }

    let columns: Vec<String> = {
        let mut cols: Vec<String> = ba.ctx.data_rows[0].keys().cloned().collect();
        cols.sort();
        cols
    };

    let mut all_rows: Vec<AnyView> = Vec::new();

    let header_cells: Vec<AnyView> = columns
        .iter()
        .map(|col| {
            AnyView::new(
                text(col.clone())
                    .size(11.0)
                    .bold()
                    .foreground(Color::srgb_hex("#808080")),
            )
        })
        .collect();
    all_rows.push(AnyView::new(hstack(header_cells).spacing(8.0)));

    for row in &ba.ctx.data_rows {
        let cells: Vec<AnyView> = columns
            .iter()
            .map(|col| {
                let val = row
                    .get(col)
                    .map(|v| v.to_display_string())
                    .unwrap_or_default();
                AnyView::new(text(val).size(13.0))
            })
            .collect();
        all_rows.push(AnyView::new(hstack(cells).spacing(8.0)));
    }

    AnyView::new(vstack(all_rows).spacing(2.0))
}
