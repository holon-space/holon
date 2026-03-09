use super::prelude::*;

pub fn build(ba: BA<'_>) -> Element {
    let message = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("message"))
        .unwrap_or("Unknown error")
        .to_string();

    rsx! {
        div {
            display: "flex",
            flex_direction: "row",
            align_items: "center",
            gap: "8px",
            padding: "12px",
            background_color: "rgba(255, 82, 82, 0.1)",
            border: "1px solid rgba(255, 82, 82, 0.3)",
            border_radius: "8px",

            span { font_size: "16px", color: "var(--error)", "\u{26A0}" }
            span { font_size: "13px", color: "var(--error)", {message} }
        }
    }
}
