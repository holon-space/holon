use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let title = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let mut children = vec![DisplayNode::leaf("text", Value::String(title))];

    if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        if ba.ctx.data_rows.is_empty() {
            children.push((ba.interpret)(tmpl, ba.ctx));
        } else {
            for row in &ba.ctx.data_rows {
                let row_ctx = ba.ctx.with_row(row.clone());
                children.push((ba.interpret)(tmpl, &row_ctx));
            }
        }
    } else {
        for expr in &ba.args.positional_exprs {
            children.push((ba.interpret)(expr, ba.ctx));
        }
    }

    DisplayNode::layout("section", children)
}
