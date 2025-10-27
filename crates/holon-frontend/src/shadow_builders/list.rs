use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let items = match template {
        Some(tmpl) => {
            if ba.ctx.data_rows.is_empty() {
                vec![(ba.interpret)(tmpl, ba.ctx)]
            } else {
                ba.ctx
                    .data_rows
                    .iter()
                    .map(|row| {
                        let row_ctx = ba.ctx.with_row(row.clone());
                        (ba.interpret)(tmpl, &row_ctx)
                    })
                    .collect()
            }
        }
        None => ba
            .ctx
            .data_rows
            .iter()
            .map(|row| DisplayNode::element("row", row.clone(), vec![]))
            .collect(),
    };

    DisplayNode::collection("list", items)
}
