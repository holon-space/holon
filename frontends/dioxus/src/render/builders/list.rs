use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let views: Vec<Element> = if let Some(tmpl) = template {
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
                let text = format!("{key}: {}", value.to_display_string());
                rsx! { span { font_size: "13px", color: "var(--text-muted)", {text} } }
            })
            .collect()
    };

    rsx! {
        div { display: "flex", flex_direction: "column", gap: "4px",
            {views.into_iter()}
        }
    }
}
