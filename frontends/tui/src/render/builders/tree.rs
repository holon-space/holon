use super::prelude::*;
use holon_frontend::render_interpreter::shared_tree_build;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let items = shared_tree_build(&ba);

    if items.is_empty() {
        return TuiWidget::Text {
            content: "[tree: no item_template]".to_string(),
            bold: false,
        };
    }

    let mut children = Vec::new();
    for (widget, depth) in items {
        let indent = "  ".repeat(depth);
        if indent.is_empty() {
            children.push(widget);
        } else {
            children.push(TuiWidget::Row {
                children: vec![
                    TuiWidget::Text {
                        content: indent,
                        bold: false,
                    },
                    widget,
                ],
            });
        }
    }

    TuiWidget::Column { children }
}
