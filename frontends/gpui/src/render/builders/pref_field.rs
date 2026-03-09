use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let key = ba.args.get_string("key").unwrap_or("unknown");
    let pref_type = ba.args.get_string("pref_type").unwrap_or("text");
    let requires_restart = ba.args.get_bool("requires_restart").unwrap_or(false);

    let empty = std::collections::HashMap::new();
    let row = ba
        .ctx
        .data_rows
        .iter()
        .find(|r| r.get("key").and_then(|v| v.as_string()) == Some(key))
        .unwrap_or_else(|| ba.ctx.data_rows.first().unwrap_or(&empty));
    let label = row
        .get("label")
        .and_then(|v| match v {
            holon_api::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .unwrap_or(key);
    let value = row
        .get("value")
        .and_then(|v| match v {
            holon_api::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .unwrap_or("");

    let label_el = div()
        .flex_col()
        .gap_1()
        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(label.to_string()));

    let description = row
        .get("description")
        .and_then(|v| match v {
            holon_api::Value::String(s) => Some(s.as_str()),
            _ => None,
        });
    let label_el = if let Some(desc) = description {
        label_el.child(
            div()
                .text_sm()
                .text_color(tc(&ba, |t| t.text_secondary))
                .child(desc.to_string()),
        )
    } else {
        label_el
    };

    let input_el = match pref_type {
        "choice" => {
            // Render current selection as text (full interactive dropdown is frontend-native)
            div()
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .bg(tc(&ba, |t| t.background_secondary))
                .border_1()
                .border_color(tc(&ba, |t| t.border))
                .child(if value.is_empty() { "Select..." } else { value }.to_string())
        }
        "secret" => {
            let masked = if value.is_empty() {
                "Not set".to_string()
            } else {
                "••••••••".to_string()
            };
            div()
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .bg(tc(&ba, |t| t.background_secondary))
                .border_1()
                .border_color(tc(&ba, |t| t.border))
                .child(masked)
        }
        "toggle" => {
            let checked = row
                .get("value")
                .and_then(|v| match v {
                    holon_api::Value::Boolean(b) => Some(*b),
                    _ => None,
                })
                .unwrap_or(false);
            let symbol = if checked { "[x]" } else { "[ ]" };
            let color = if checked {
                tc(&ba, |t| t.success)
            } else {
                tc(&ba, |t| t.text_secondary)
            };
            div().child(symbol).text_color(color)
        }
        "directory_path" => div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(tc(&ba, |t| t.background_secondary))
            .border_1()
            .border_color(tc(&ba, |t| t.border))
            .child(if value.is_empty() { "No directory selected" } else { value }.to_string()),
        // "text" and fallback
        _ => div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(tc(&ba, |t| t.background_secondary))
            .border_1()
            .border_color(tc(&ba, |t| t.border))
            .child(value.to_string()),
    };

    let mut container = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .py_2()
        .child(label_el)
        .child(input_el);

    if requires_restart {
        container = container.child(
            div()
                .text_xs()
                .text_color(tc(&ba, |t| t.warning))
                .child("Requires restart"),
        );
    }

    container
}
