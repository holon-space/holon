use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let content = ba.args.get_string("content").unwrap_or("").to_string();

    DisplayNode::element(
        "source_editor",
        [
            ("language".into(), Value::String(language)),
            ("content".into(), Value::String(content)),
        ]
        .into(),
        vec![],
    )
}
