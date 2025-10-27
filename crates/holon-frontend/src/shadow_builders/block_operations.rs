use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let op_names: Vec<String> = ba
        .ctx
        .operations
        .iter()
        .map(|ow| ow.descriptor.name.clone())
        .collect();

    DisplayNode::element(
        "block_operations",
        [(
            "operations".into(),
            Value::String(op_names.join(",")),
        )]
        .into(),
        vec![],
    )
}
