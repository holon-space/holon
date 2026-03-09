use super::prelude::*;
use holon_frontend::render_interpreter::{shared_render_entity_build, RenderBlockResult};

pub fn build(ba: BA<'_>) -> Element {
    match shared_render_entity_build(&ba) {
        RenderBlockResult::ProfileWidget { render, operations } => {
            let ctx = ba.ctx.with_operations(operations);
            (ba.interpret)(&render, &ctx)
        }
        RenderBlockResult::Empty => rsx! {},
        RenderBlockResult::Error(msg) => {
            rsx! { span { font_size: "12px", color: "var(--error)", {msg} } }
        }
    }
}
