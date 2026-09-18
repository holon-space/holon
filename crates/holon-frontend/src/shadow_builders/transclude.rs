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

        // The target is authored text as a positional argument and a row's own
        // column as `target_uri`, so it is resolved by the one classifier: a
        // value that forms no URI is a fault to name, never a reason to unwind.
        let block_id = match holon_api::row_id_of_str(&uri) {
            holon_api::RowId::Entity(block_id) if block_id.is_block() => block_id,
            holon_api::RowId::Entity(_) => {
                let mut __props = std::collections::HashMap::new();
                __props.insert(
                    "content".to_string(),
                    Value::String(format!("[transclude: {uri}]")),
                );
                __props.insert("bold".to_string(), Value::Boolean(false));
                __props.insert("size".to_string(), Value::Float(14.0));
                __props.insert("color".to_string(), Value::String("muted".to_string()));
                return ViewModel::from_widget("text", __props);
            }
            holon_api::RowId::Unusable(refusal) => {
                return ViewModel::error("transclude", refusal.to_string());
            }
            holon_api::RowId::Absent => {
                return ViewModel::error("transclude", "transclude: missing URI argument");
            }
        };

        // Placeholder — resolved reactively by the frontend or via snapshot_resolved.
        ViewModel::live_block(block_id)
    }
}
