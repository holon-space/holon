use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let mut children = Vec::new();

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        children.push((ba.interpret)(template, ba.ctx));
    }

    for val in &ba.args.positional {
        if let holon_api::Value::String(s) = val {
            children.push(TuiWidget::Text {
                content: s.clone(),
                bold: false,
            });
        }
    }

    TuiWidget::Row { children }
}
