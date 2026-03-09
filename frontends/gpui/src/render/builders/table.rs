use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let muted = tc(&ba, |t| t.text_secondary);
    let mut container = div().flex_col().gap_0p5();

    if ba.ctx.data_rows.is_empty() {
        return container.child(div().text_sm().text_color(muted).child("[empty]"));
    }

    let columns: Vec<String> = {
        let mut cols: Vec<String> = ba.ctx.data_rows[0].keys().cloned().collect();
        cols.sort();
        cols
    };

    let mut header = div().flex().flex_row().gap_2();
    for col in &columns {
        header = header.child(
            div()
                .w(px(120.0))
                .text_xs()
                .text_color(muted)
                .child(col.clone()),
        );
    }
    container = container.child(header);

    for row in &ba.ctx.data_rows {
        let mut row_div = div().flex().flex_row().gap_2();
        for col in &columns {
            let val = row
                .get(col)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            row_div = row_div.child(div().w(px(120.0)).text_sm().child(val));
        }
        container = container.child(row_div);
    }

    container
}
