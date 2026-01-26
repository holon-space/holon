use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let message = ba
        .args
        .get_string("message")
        .or_else(|| ba.args.get_positional_string(0))
        .unwrap_or("Unknown error")
        .to_string();

    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(&ba, |t| t.background_secondary))
        .text_color(tc(&ba, |t| t.error))
        .text_sm()
        .child(message)
}
