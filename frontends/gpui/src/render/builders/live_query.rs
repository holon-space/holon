use super::prelude::*;
use holon_frontend::render_interpreter::shared_live_query_build;

pub fn build(ba: BA<'_>) -> Div {
    match shared_live_query_build(&ba) {
        Ok(widget) => widget,
        Err(msg) => div()
            .p_1()
            .rounded(px(4.0))
            .bg(tc(&ba, |t| t.background_secondary))
            .text_color(tc(&ba, |t| t.error))
            .child(msg),
    }
}
