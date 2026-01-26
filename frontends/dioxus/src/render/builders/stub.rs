use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        let views: Vec<Element> = if ba.ctx.data_rows.is_empty() {
            vec![(ba.interpret)(tmpl, ba.ctx)]
        } else {
            ba.ctx
                .data_rows
                .iter()
                .map(|row| (ba.interpret)(tmpl, &ba.ctx.with_row(row.clone())))
                .collect()
        };
        rsx! {
            div { display: "flex", flex_direction: "column",
                {views.into_iter()}
            }
        }
    } else {
        rsx! {
            span { font_size: "12px", color: "var(--text-muted)", "[stub]" }
        }
    }
}
