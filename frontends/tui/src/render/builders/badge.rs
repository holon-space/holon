use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let content = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("content"))
        .map(|s| s.to_string())
        .unwrap_or_default();

    TuiWidget::Badge { content }
}
