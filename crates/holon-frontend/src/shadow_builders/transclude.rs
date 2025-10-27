use holon_api::EntityUri;

use super::prelude::*;

holon_macros::widget_builder! {
    raw fn transclude(ba: BA<'_>) -> ViewModel {
        let uri = ba
            .args
            .get_positional_string(0)
            .or_else(|| ba.ctx.row().get("target_uri").and_then(|v| v.as_string()))
            .unwrap_or("")
            .to_string();

        if uri.is_empty() {
            return ViewModel::error("transclude", "transclude: missing URI argument");
        }

        let block_id = if uri.starts_with("block:") {
            EntityUri::parse(&uri).expect("transclude: invalid block URI")
        } else {
            return ViewModel::from_kind(NodeKind::Text {
                content: format!("[transclude: {uri}]"),
                bold: false,
                size: 14.0,
                color: Some("muted".to_string()),
            });
        };

        let (render_expr, data_rows) = ba.ctx.block_watch().get_or_watch(&block_id);
        let child_ctx = ba.ctx.deeper_query().with_data_rows(data_rows);
        let interp = crate::create_shadow_interpreter();
        let node = interp.interpret(&render_expr, &child_ctx);
        ViewModel::block_ref(block_id, node)
    }
}
