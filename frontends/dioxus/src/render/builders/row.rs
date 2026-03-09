use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let mut views: Vec<Element> = Vec::new();

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        views.push((ba.interpret)(template, ba.ctx));
    }

    for val in &ba.args.positional {
        if let Value::String(s) = val {
            let s = s.clone();
            views.push(rsx! { span { font_size: "14px", {s} } });
        }
    }

    rsx! {
        div { display: "flex", flex_direction: "row", gap: "8px", align_items: "center",
            {views.into_iter()}
        }
    }
}
