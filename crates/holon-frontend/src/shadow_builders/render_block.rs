use super::prelude::*;
use crate::render_interpreter::shared_render_block_build;

holon_macros::widget_builder! {
    raw fn render_block(ba: BA<'_>) -> ViewModel {
        match shared_render_block_build(&ba) {
            crate::render_interpreter::RenderBlockResult::Widget(w) => w,
            crate::render_interpreter::RenderBlockResult::SourceBlock { language, content } => {
                ViewModel::element(
                    "source_block",
                    [
                        ("language".into(), Value::String(language)),
                        ("content".into(), Value::String(content)),
                    ]
                    .into(),
                    vec![],
                )
            }
            crate::render_interpreter::RenderBlockResult::TextContent { content, operations } => {
                let ctx = ba.ctx.with_operations(operations);
                ViewModel {
                    entity: ctx.row().clone(),
                    kind: NodeKind::EditableText {
                        content,
                        field: "content".to_string(),
                    },
                    operations: ctx.operations.clone(),
                    triggers: ctx.triggers.clone(),
                    ..Default::default()
                }
            }
            crate::render_interpreter::RenderBlockResult::ProfileWidget { render, operations } => {
                let ctx = ba.ctx.with_operations(operations);
                (ba.interpret)(&render, &ctx)
            }
            crate::render_interpreter::RenderBlockResult::Empty => ViewModel::empty(),
            crate::render_interpreter::RenderBlockResult::Error(msg) => {
                ViewModel::error("render_block", msg)
            }
        }
    }
}
