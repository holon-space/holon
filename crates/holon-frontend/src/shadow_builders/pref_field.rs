use super::prelude::*;

/// Shadow builder for a single preference field.
///
/// Dispatches based on `pref_type` named arg to produce the appropriate
/// leaf widget representation in the shadow DOM. The actual interactive
/// behavior lives in each frontend's native builder; this shadow builder
/// provides the structure for testing and headless rendering.
pub fn build(ba: BA<'_>) -> DisplayNode {
    let key = ba
        .args
        .get_string("key")
        .unwrap_or("unknown")
        .to_string();
    let pref_type = ba
        .args
        .get_string("pref_type")
        .unwrap_or("text")
        .to_string();
    let requires_restart = ba.args.get_bool("requires_restart").unwrap_or(false);

    let empty = std::collections::HashMap::new();
    let row = ba
        .ctx
        .data_rows
        .iter()
        .find(|r| r.get("key").and_then(|v| v.as_string()) == Some(&key))
        .unwrap_or_else(|| ba.ctx.data_rows.first().unwrap_or(&empty));
    let value = row
        .get("value")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let label = row
        .get("label")
        .and_then(|v| match v {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .unwrap_or(&key)
        .to_string();

    let mut children = vec![DisplayNode::leaf("text", Value::String(label))];

    let widget_type = match pref_type.as_str() {
        "choice" => "dropdown",
        "secret" => "secret_text",
        "text" => "editable_text",
        "toggle" => "checkbox",
        "directory_path" => "platform_action",
        _ => "editable_text",
    };

    children.push(DisplayNode::leaf(widget_type, value.clone()));

    if requires_restart {
        children.push(DisplayNode::leaf("text", Value::String("Requires restart".into())));
    }

    let mut data = std::collections::HashMap::new();
    data.insert("key".into(), Value::String(key));
    data.insert("pref_type".into(), Value::String(pref_type));
    data.insert("value".into(), value);

    DisplayNode::element("pref_field", data, children)
}
