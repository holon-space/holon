use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let label = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("label"))
        .unwrap_or("")
        .to_string();

    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(12.0))
        .bg(tc(&ba, |t| t.background_secondary))
        .text_sm()
        .text_color(tc(&ba, |t| t.text_secondary))
        .child(label)
}
