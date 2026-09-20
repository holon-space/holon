use super::prelude::*;
use crate::render_interpreter::shared_live_query_build;

holon_macros::widget_builder! {
    raw fn live_query(ba: BA<'_>) -> ViewModel {
        // Becomes a typed `Expr` param when `live_query` migrates off `raw fn`.
        let __item_template = ba.args.get_template("item_template")
            .or(ba.args.get_template("item"));
        match shared_live_query_build(&ba, __item_template) {
            Ok(result) => {
                // Every `RenderExpr` has a JSON form: its maps are keyed by
                // String, and `dynamic_to_value` refuses the one scalar that
                // has none (a non-finite float).
                let __render_expr_json = serde_json::to_string(&result.render_expr)
                    .expect("a RenderExpr always serialises");
                let mut __props = std::collections::HashMap::new();
                // The spec decides which props the node carries, so the
                // platform layer reads one arm or the other and never has to
                // guess which subscription a node wants.
                match &result.spec {
                    holon_api::row_source::RowSourceSpec::Query { lang, text, .. } => {
                        __props.insert("query".to_string(), Value::String(text.clone()));
                        __props.insert("query_lang".to_string(),
                            Value::String(lang.to_string()));
                    }
                    holon_api::row_source::RowSourceSpec::Named(named) => {
                        __props.insert("source".to_string(),
                            Value::String(named.name().as_str().to_string()));
                        if let Some(filter) = named.filter() {
                            __props.insert("where_column".to_string(),
                                Value::String(filter.column().as_str().to_string()));
                            __props.insert("where_equals".to_string(),
                                Value::String(filter.equals().to_string()));
                        }
                    }
                }
                if let Some(ref ctx_id) = result.query_context_id {
                    __props.insert("query_context_id".to_string(), Value::String(ctx_id.clone()));
                }
                __props.insert("render_expr".to_string(), Value::String(__render_expr_json));
                ViewModel {
                    slot: Some(crate::reactive_view_model::ReactiveSlot::new(result.content)),
                    ..ViewModel::from_widget("live_query", __props)
                }
            }
            Err(msg) => ViewModel::error("live_query", msg),
        }
    }
}
