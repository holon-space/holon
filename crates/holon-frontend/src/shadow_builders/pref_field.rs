use super::prelude::*;

holon_macros::widget_builder! {
    raw fn pref_field(ba: BA<'_>) -> ViewModel {
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

        let empty = holon_api::widget_spec::DataRow::new();
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

        let mut children = vec![ViewModel::leaf("text", Value::String(label))];

        let widget_type = match pref_type.as_str() {
            "choice" => "dropdown",
            "secret" => "secret_text",
            "text" => "editable_text",
            "toggle" => "checkbox",
            "directory_path" => "platform_action",
            _ => "editable_text",
        };

        children.push(ViewModel::leaf(widget_type, value.clone()));

        if requires_restart {
            children.push(ViewModel::leaf("text", Value::String("Requires restart".into())));
        }

        let mut data = std::collections::HashMap::new();
        data.insert("key".into(), Value::String(key));
        data.insert("pref_type".into(), Value::String(pref_type));
        data.insert("value".into(), value);
        if let Some(options) = row.get("options").cloned() {
            data.insert("options".into(), options);
        }

        ViewModel::element("pref_field", data, children)
    }
}
