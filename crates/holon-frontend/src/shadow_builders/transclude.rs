use holon_api::EntityUri;

use super::prelude::*;

holon_macros::widget_builder! {
    raw fn transclude(ba: BA<'_>) -> ViewModel {
        let uri = ba
            .args
            .get_positional_string(0)
            .or_else(|| ba.ctx.row().get("target_uri").and_then(|v| v.as_string()).map(|s| s.to_string()))
            .unwrap_or_default();

        if uri.is_empty() {
            return ViewModel::error("transclude", "transclude: missing URI argument");
        }

        let block_id = if uri.starts_with("block:") {
            EntityUri::parse(&uri).expect("transclude: invalid block URI")
        } else {
            let mut __props = std::collections::HashMap::new();
            __props.insert("content".to_string(), Value::String(format!("[transclude: {uri}]")));
            __props.insert("bold".to_string(), Value::Boolean(false));
            __props.insert("size".to_string(), Value::Float(14.0));
            __props.insert("color".to_string(), Value::String("muted".to_string()));
            return ViewModel::from_widget("text", __props);
        };

        // Placeholder — resolved reactively by the frontend or via snapshot_resolved.
        ViewModel::live_block(block_id)
    }
}
