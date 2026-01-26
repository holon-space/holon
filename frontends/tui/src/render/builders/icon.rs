use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let symbol = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("source").map(str::to_string))
        .unwrap_or_else(|| "●".to_string());

    TuiWidget::Icon { symbol }
}
