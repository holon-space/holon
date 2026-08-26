use std::collections::HashMap;

use super::prelude::*;

holon_macros::widget_builder! {
    raw fn query_result(ba: BA<'_>) -> ViewModel {
        let result_value = ba
            .ctx
            .row()
            .get("result")
            .or_else(|| ba.ctx.row().get("results"))
            .cloned()
            .unwrap_or(Value::Null);

        match &result_value {
            Value::Null => ViewModel::leaf("text", Value::String("[no result]".into())),
            Value::String(s) if s.is_empty() => {
                ViewModel::leaf("text", Value::String("[empty result]".into()))
            }
            Value::String(s) => parse_json_rows(s),
            Value::Json(j) => parse_json_rows(j),
            other => ViewModel::leaf("text", other.clone()),
        }
    }
}

/// A table cell's DISPLAY text — deliberately NOT the JSON→`Value` parse.
///
/// Every non-null cell renders as text, so a number, a bool and a nested blob
/// all arrive at the renderer already stringified. Resembling a parser is what
/// made this look like a sixth copy of one; it is a formatter, and unifying it
/// with `Value::from_json_value` would change what the table renders.
fn cell_text(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::String(s) => Value::String(s.clone()),
        other => Value::String(other.to_string()),
    }
}

fn parse_json_rows(s: &str) -> ViewModel {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(serde_json::Value::Array(rows)) => {
            let items: Vec<ViewModel> = rows
                .iter()
                .filter_map(|row| {
                    if let serde_json::Value::Object(map) = row {
                        let data: HashMap<String, Value> =
                            map.iter().map(|(k, v)| (k.clone(), cell_text(v))).collect();
                        Some(ViewModel::element("table_row", Arc::new(data), vec![]))
                    } else {
                        None
                    }
                })
                .collect();
            ViewModel::static_collection("query_result", items, 4.0, false, Default::default())
        }
        _ => ViewModel::leaf("text", Value::String(s.to_string())),
    }
}
