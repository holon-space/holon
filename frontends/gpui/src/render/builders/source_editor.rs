use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let language = ba.args.get_string("language").unwrap_or("text").to_string();
    let content = ba.args.get_string("content").unwrap_or("").to_string();

    div()
        .size_full()
        .p_2()
        .bg(tc(&ba, |t| t.background_secondary))
        .text_xs()
        .flex_col()
        .child(
            div()
                .text_color(tc(&ba, |t| t.text_tertiary))
                .text_xs()
                .child(language),
        )
        .child(div().child(content))
}
