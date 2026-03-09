use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let indent = format!("{}px", ba.ctx.depth * 29);

    let child = if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(template, ba.ctx)
    } else {
        rsx! {}
    };

    rsx! {
        div { display: "flex", flex_direction: "column", padding_left: "{indent}",
            {child}
        }
    }
}
