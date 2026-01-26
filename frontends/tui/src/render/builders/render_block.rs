use super::prelude::*;
use holon_frontend::render_interpreter::{shared_render_block_build, RenderBlockResult};

pub fn build(ba: BA<'_>) -> TuiWidget {
    match shared_render_block_build(&ba) {
        RenderBlockResult::ProfileWidget { render, operations } => {
            let ctx = ba.ctx.with_operations(operations, ba.services);
            (ba.interpret)(&render, &ctx)
        }
        RenderBlockResult::Empty => TuiWidget::Empty,
        RenderBlockResult::Error(msg) => TuiWidget::Text {
            content: format!("[error: {}]", msg),
            bold: false,
        },
    }
}
