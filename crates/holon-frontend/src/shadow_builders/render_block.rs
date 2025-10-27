use super::prelude::*;
use crate::render_interpreter::shared_render_block_build;

pub fn build(ba: BA<'_>) -> DisplayNode {
    match shared_render_block_build(&ba) {
        crate::render_interpreter::RenderBlockResult::Widget(w) => w,
        crate::render_interpreter::RenderBlockResult::SourceBlock { language, content } => {
            DisplayNode::element(
                "source_block",
                [
                    ("language".into(), Value::String(language)),
                    ("content".into(), Value::String(content)),
                ]
                .into(),
                vec![],
            )
        }
        crate::render_interpreter::RenderBlockResult::TextContent(content) => {
            DisplayNode::leaf("text", Value::String(content))
        }
        crate::render_interpreter::RenderBlockResult::Empty => DisplayNode::EMPTY,
        crate::render_interpreter::RenderBlockResult::Error(msg) => {
            DisplayNode::error("render_block", msg)
        }
    }
}
