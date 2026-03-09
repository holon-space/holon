use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let title = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let mut views: Vec<Element> = Vec::new();

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

    rsx! {
        div { padding: "8px", display: "flex", flex_direction: "column", gap: "8px",
            span { font_size: "18px", font_weight: "bold", {title} }
            {views.into_iter()}
        }
    }
}
