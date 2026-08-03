use super::prelude::*;
use crate::render_interpreter::shared_live_query_build;

holon_macros::widget_builder! {
    raw fn live_query(ba: BA<'_>) -> ViewModel {
        // Becomes a typed `Expr` param when `live_query` migrates off `raw fn`.
        let __item_template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"));
        match shared_live_query_build(&ba, __item_template) {
            Ok(result) => {
                let mut __props = std::collections::HashMap::new();
                __props.insert("query".to_string(), Value::String(result.query));
                __props.insert("query_lang".to_string(),
                    Value::String(result.query_lang.to_string()));
                if let Some(ref ctx_id) = result.query_context_id {
                    __props.insert("query_context_id".to_string(), Value::String(ctx_id.clone()));
                }
                __props.insert("render_expr".to_string(),
                    Value::String(serde_json::to_string(&result.render_expr).unwrap_or_default()));
                ViewModel {
                    slot: Some(crate::reactive_view_model::ReactiveSlot::new(result.content)),
                    ..ViewModel::from_widget("live_query", __props)
                }
            }
            Err(msg) => ViewModel::error("live_query", msg),
        }
    }
}
