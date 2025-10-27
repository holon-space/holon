use holon_api::EntityUri;

use super::prelude::*;

holon_macros::widget_builder! {
    raw fn block_ref(ba: BA<'_>) -> ViewModel {
        let block_id = ba
            .args
            .get_positional_string(0)
            .or_else(|| ba.ctx.row().get("id").and_then(|v| v.as_string()))
            .map(|s| EntityUri::parse(&s).expect("block_ref: invalid entity URI"))
            .expect("block_ref: no positional arg and no 'id' column in current row");

        if ba.ctx.query_depth >= 10 {
            return ViewModel::error(
                "block_ref",
                format!("[block_ref recursion limit reached (depth {})]", ba.ctx.query_depth),
            );
        }

        let (render_expr, data_rows) = ba.ctx.block_watch().get_or_watch(&block_id);
        let child_ctx = ba.ctx.deeper_query().with_data_rows(data_rows);
        let interp = crate::create_shadow_interpreter();
        let node = interp.interpret(&render_expr, &child_ctx);
        ViewModel::block_ref(block_id, node)
    }
}
