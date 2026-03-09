use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let content = ba
        .args
        .get_positional_string(0)
        .map(|s| s.to_string())
        .or_else(|| ba.args.get_string("content").map(|s| s.to_string()))
        .unwrap_or_else(|| {
            ba.args
                .positional
                .first()
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        });

    let bold = ba.args.get_bool("bold").unwrap_or(false);

    let mut el = div().child(content);
    if bold {
        el = el.font_weight(gpui::FontWeight::BOLD);
    }
    el
}
