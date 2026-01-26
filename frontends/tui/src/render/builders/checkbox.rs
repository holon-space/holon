use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let checked = ba
        .args
        .get_bool("checked")
        .or_else(|| {
            ba.args
                .positional
                .first()
                .and_then(|v| match v {
                    holon_api::Value::Boolean(b) => Some(*b),
                    holon_api::Value::Integer(i) => Some(*i != 0),
                    _ => None,
                })
        })
        .unwrap_or(false);

    TuiWidget::Checkbox { checked }
}
