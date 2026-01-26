use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let mut container = div().flex().flex_row().gap_2().items_center();

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        container = container.child((ba.interpret)(template, ba.ctx));
    }

    for val in &ba.args.positional {
        if let holon_api::Value::String(s) = val {
            container = container.child(s.clone());
        }
    }

    container
}
