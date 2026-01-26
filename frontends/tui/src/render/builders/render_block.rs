use super::prelude::*;
use holon_frontend::render_interpreter::{shared_render_block_build, RenderBlockResult};

pub fn build(ba: BA<'_>) -> TuiWidget {
    match shared_render_block_build(&ba) {
        RenderBlockResult::Widget(w) => w,
        RenderBlockResult::SourceBlock { language, content } => TuiWidget::Column {
            children: vec![
                TuiWidget::Text {
                    content: format!("```{}", language),
                    bold: true,
                },
                TuiWidget::Text {
                    content,
                    bold: false,
                },
                TuiWidget::Text {
                    content: "```".to_string(),
                    bold: false,
                },
            ],
        },
        RenderBlockResult::TextContent { content, .. } => TuiWidget::Text {
            content,
            bold: false,
        },
        RenderBlockResult::ProfileWidget { render, operations } => {
            let ctx = ba.ctx.with_operations(operations);
            (ba.interpret)(&render, &ctx)
        }
        RenderBlockResult::Empty => TuiWidget::Empty,
        RenderBlockResult::Error(msg) => TuiWidget::Text {
            content: format!("[error: {}]", msg),
            bold: false,
        },
    }
}
