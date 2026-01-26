use super::prelude::*;
use holon_api::Value;

/// query_result() — read-only display of a result/results column.
pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    let result_value = ctx
        .row()
        .get("result")
        .or_else(|| ctx.row().get("results"))
        .cloned()
        .unwrap_or(Value::Null);

    match &result_value {
        Value::Null => div().child(
            text("[no result]")
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        ),
        Value::String(s) if s.is_empty() => div().child(
            text("[empty result]")
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        ),
        Value::String(s) => {
            // Try parsing as JSON array-of-objects for table rendering
            if let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(s) {
                render_table(&rows, &theme)
            } else {
                div().child(
                    text(s.clone())
                        .size(13.0)
                        .color(theme.color(ColorToken::TextPrimary)),
                )
            }
        }
        Value::Json(j) => {
            if let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(j) {
                render_table(&rows, &theme)
            } else {
                div().child(
                    text(j.clone())
                        .size(13.0)
                        .color(theme.color(ColorToken::TextPrimary)),
                )
            }
        }
        other => div().child(
            text(format!("{other:?}"))
                .size(13.0)
                .color(theme.color(ColorToken::TextPrimary)),
        ),
    }
}

fn render_table(rows: &[serde_json::Value], theme: &ThemeState) -> Div {
    if rows.is_empty() {
        return div().child(
            text("[empty result set]")
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        );
    }

    // Extract column names from first row
    let columns: Vec<String> = match &rows[0] {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => {
            return div().child(
                text("[non-tabular result]")
                    .size(12.0)
                    .color(theme.color(ColorToken::TextSecondary)),
            );
        }
    };

    let mut container = div().flex_col().gap(1.0);

    // Header row
    let mut header = div().flex_row().gap(8.0);
    for col in &columns {
        header = header.child(
            div().w(120.0).child(
                text(col.clone())
                    .size(12.0)
                    .color(theme.color(ColorToken::TextSecondary)),
            ),
        );
    }
    container = container.child(header);

    // Data rows
    for row in rows {
        let mut row_div = div().flex_row().gap(8.0);
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
                row_div = row_div.child(
                    div().w(120.0).child(
                        text(val)
                            .size(13.0)
                            .color(theme.color(ColorToken::TextPrimary)),
                    ),
                );
            }
        }
        container = container.child(row_div);
    }

    container
}
