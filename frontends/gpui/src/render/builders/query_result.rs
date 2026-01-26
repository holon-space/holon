use super::prelude::*;
use holon_api::Value;

pub fn build(ba: BA<'_>) -> Div {
    let muted = tc(&ba, |t| t.text_secondary);

    let result_value = ba
        .ctx
        .row()
        .get("result")
        .or_else(|| ba.ctx.row().get("results"))
        .cloned()
        .unwrap_or(Value::Null);

    match &result_value {
        Value::Null => div().text_sm().text_color(muted).child("[no result]"),
        Value::String(s) if s.is_empty() => {
            div().text_sm().text_color(muted).child("[empty result]")
        }
        Value::String(s) => {
            if let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(s) {
                render_table(&rows, muted)
            } else {
                div().text_sm().child(s.clone())
            }
        }
        Value::Json(j) => {
            if let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(j) {
                render_table(&rows, muted)
            } else {
                div().text_sm().child(j.clone())
            }
        }
        other => div().text_sm().child(format!("{other:?}")),
    }
}

fn render_table(rows: &[serde_json::Value], muted: Rgba) -> Div {
    if rows.is_empty() {
        return div()
            .text_sm()
            .text_color(muted)
            .child("[empty result set]");
    }

    let columns: Vec<String> = match &rows[0] {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => {
            return div()
                .text_sm()
                .text_color(muted)
                .child("[non-tabular result]");
        }
    };

    let mut container = div().flex_col().gap_px();

    let mut header = div().flex().flex_row().gap_2();
    for col in &columns {
        header = header.child(
            div()
                .w(px(120.0))
                .text_xs()
                .text_color(muted)
                .child(col.clone()),
        );
    }
    container = container.child(header);

    for row in rows {
        let mut row_div = div().flex().flex_row().gap_2();
        if let serde_json::Value::Object(map) = row {
            for col in &columns {
                let val = map
                    .get(col)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                row_div = row_div.child(div().w(px(120.0)).text_sm().child(val));
            }
        }
        container = container.child(row_div);
    }

    container
}
