use super::prelude::*;
use holon_frontend::render_interpreter::{shared_render_block_build, RenderBlockResult};

pub fn build(ba: BA<'_>) -> Element {
    match shared_render_block_build(&ba) {
        RenderBlockResult::Widget(w) => w,
        RenderBlockResult::SourceBlock { language, content } => {
            rsx! {
                div { display: "flex", flex_direction: "column", gap: "2px",
                    span { font_size: "10px", color: "var(--text-muted)", "[{language}]" }
                    pre { font_size: "13px", padding: "4px", {content} }
                }
            }
        }
        RenderBlockResult::TextContent(content) => {
            rsx! { span { font_size: "14px", {content} } }
        }
        RenderBlockResult::Empty => rsx! {},
        RenderBlockResult::Error(msg) => {
            rsx! { span { font_size: "12px", color: "var(--error)", {msg} } }
        }
    }
}
