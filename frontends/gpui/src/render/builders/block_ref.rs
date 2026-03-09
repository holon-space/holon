use super::prelude::*;
use crate::render::tracked_div::TrackedDiv;

pub fn build(ba: BA<'_>) -> Div {
    let block_id = match ba.ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            return div()
                .text_color(tc(&ba, |t| t.error))
                .child("[block_ref: no id in row]");
        }
    };

    let content = match ba.ctx.block_cache.get_or_watch(&block_id) {
        Some((render_expr, data_rows)) => {
            let child_ctx = ba.ctx.deeper_query().with_data_rows(data_rows);
            (ba.interpret)(&render_expr, &child_ctx)
        }
        None => div()
            .text_color(tc(&ba, |t| t.text_tertiary))
            .child("Loading..."),
    };

    let tracked = TrackedDiv::new(block_id, ba.ctx.ext.clone(), content);
    div().child(tracked)
}
