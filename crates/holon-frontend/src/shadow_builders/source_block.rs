use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let source = ba
        .args
        .get_string("source")
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();
    let name = ba.args.get_string("name").unwrap_or("").to_string();

    DisplayNode::element(
        "source_block",
        [
            ("language".into(), Value::String(language)),
            ("content".into(), Value::String(source)),
            ("name".into(), Value::String(name)),
        ]
        .into(),
        vec![],
    )
}
