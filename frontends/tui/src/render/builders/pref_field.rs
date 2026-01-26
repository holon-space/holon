use super::prelude::*;

pub fn build(ba: BA<'_>) -> TuiWidget {
    let key = ba.args.get_string("key").unwrap_or("unknown");
    let pref_type = ba.args.get_string("pref_type").unwrap_or("text");
    let requires_restart = ba.args.get_bool("requires_restart").unwrap_or(false);

    let empty = holon_api::widget_spec::DataRow::new();
    let row = ba
        .ctx
        .data_rows
        .iter()
        .find(|r| r.get("key").and_then(|v| v.as_string()) == Some(key))
        .unwrap_or_else(|| ba.ctx.data_rows.first().unwrap_or(&empty));
    let label = row
        .get("label")
        .and_then(|v| match v {
            holon_api::Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| key.to_string());
    let value = row
        .get("value")
        .and_then(|v| match v {
            holon_api::Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default();

    let value_display = match pref_type {
        "secret" => {
            if value.is_empty() {
                "(not set)".into()
            } else {
                "••••••••".into()
            }
        }
        "toggle" => {
            let checked = row
                .get("value")
                .and_then(|v| match v {
                    holon_api::Value::Boolean(ref b) => Some(*b),
                    _ => None,
                })
                .unwrap_or(false);
            if checked { "[x]" } else { "[ ]" }.into()
        }
        "directory_path" => {
            if value.is_empty() {
                "(no directory selected)".into()
            } else {
                value
            }
        }
        // "choice", "text", and fallback
        _ => {
            if value.is_empty() {
                "(not set)".into()
            } else {
                value
            }
        }
    };

    let mut children = vec![
        TuiWidget::Text {
            content: format!("{label}: "),
            bold: true,
        },
        TuiWidget::Text {
            content: value_display,
            bold: false,
        },
    ];

    if requires_restart {
        children.push(TuiWidget::Text {
            content: " (requires restart)".into(),
            bold: false,
        });
    }

    TuiWidget::Row { children }
}
