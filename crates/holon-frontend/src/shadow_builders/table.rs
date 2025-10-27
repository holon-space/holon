use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let items = ba
        .ctx
        .data_rows
        .iter()
        .map(|row| DisplayNode::element("table_row", row.clone(), vec![]))
        .collect();

    DisplayNode::collection("table", items)
}
