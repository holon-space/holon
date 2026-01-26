use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let message = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("message").map(str::to_string))
        .unwrap_or_else(|| "[error]".to_string());

    TuiWidget::Text {
        content: format!("⚠ {}", message),
        bold: true,
    }
}
