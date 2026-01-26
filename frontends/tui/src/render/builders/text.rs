use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
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

    TuiWidget::Text { content, bold }
}
