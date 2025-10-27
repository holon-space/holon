use super::prelude::*;
use crate::render_interpreter::shared_live_query_build;
use crate::view_model::NodeKind;

holon_macros::widget_builder! {
    raw fn live_query(ba: BA<'_>) -> ViewModel {
        match shared_live_query_build(&ba) {
            Ok(result) => ViewModel::from_kind(NodeKind::LiveQuery {
                content: Box::new(result.content),
                compiled_sql: Some(result.compiled_sql),
                query_context_id: result.query_context_id,
                render_expr: Some(result.render_expr),
            }),
            Err(msg) => ViewModel::error("live_query", msg),
        }
    }
}
