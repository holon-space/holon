use super::prelude::*;
use holon_frontend::render_interpreter::shared_live_query_build;

pub fn build(ba: BA<'_>) -> Element {
    match shared_live_query_build(&ba) {
        Ok(result) => result.content,
        Err(msg) => rsx! { span { font_size: "12px", color: "var(--error)", {msg} } },
    }
}
