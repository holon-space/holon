use std::collections::HashMap;

use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let result_value = ba
        .ctx
        .row()
        .get("result")
        .or_else(|| ba.ctx.row().get("results"))
        .cloned()
        .unwrap_or(Value::Null);

    match &result_value {
        Value::Null => DisplayNode::leaf("text", Value::String("[no result]".into())),
        Value::String(s) if s.is_empty() => {
            DisplayNode::leaf("text", Value::String("[empty result]".into()))
        }
        Value::String(s) => parse_json_rows(s),
        Value::Json(j) => parse_json_rows(j),
        other => DisplayNode::leaf("text", other.clone()),
    }
}

fn parse_json_rows(s: &str) -> DisplayNode {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(serde_json::Value::Array(rows)) => {
            let items: Vec<DisplayNode> = rows
                .iter()
                .filter_map(|row| {
                    if let serde_json::Value::Object(map) = row {
                        let data: HashMap<String, Value> = map
                            .iter()
                            .map(|(k, v)| {
                                let val = match v {
                                    serde_json::Value::String(s) => Value::String(s.clone()),
                                    serde_json::Value::Null => Value::Null,
                                    other => Value::String(other.to_string()),
                                };
                                (k.clone(), val)
                            })
                            .collect();
                        Some(DisplayNode::element("table_row", data, vec![]))
                    } else {
                        None
                    }
                })
                .collect();
            DisplayNode::collection("query_result", items)
        }
        _ => DisplayNode::leaf("text", Value::String(s.to_string())),
    }
}
