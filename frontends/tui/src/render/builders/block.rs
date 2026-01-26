use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let indent = "  ".repeat(ba.ctx.depth);

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        let child = (ba.interpret)(template, ba.ctx);
        if indent.is_empty() {
            child
        } else {
            TuiWidget::Row {
                children: vec![
                    TuiWidget::Text {
                        content: indent,
                        bold: false,
                    },
                    child,
                ],
            }
        }
    } else {
        TuiWidget::Empty
    }
}
