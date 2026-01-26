use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let content_type = ba
        .ctx
        .row()
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let source_language = ba
        .ctx
        .row()
        .get("source_language")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let content = ba
        .ctx
        .row()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let is_query_lang = source_language.parse::<holon_api::QueryLanguage>().is_ok();

    match (content_type.as_str(), is_query_lang) {
        ("source", true) => {
            let block_id = match ba.ctx.row().get("id").and_then(|v| v.as_string()) {
                Some(id) => id.to_string(),
                None => {
                    return div()
                        .text_color(tc(&ba, |t| t.error))
                        .child("[render_block: no id]");
                }
            };

            match ba.ctx.block_cache.get_or_watch(&block_id) {
                Some((render_expr, data_rows)) => {
                    let child_ctx = ba.ctx.deeper_query().with_data_rows(data_rows);
                    (ba.interpret)(&render_expr, &child_ctx)
                }
                None => div()
                    .text_color(tc(&ba, |t| t.text_tertiary))
                    .child("Loading..."),
            }
        }
        ("source", false) => div()
            .flex_col()
            .gap_0p5()
            .child(
                div().flex().flex_row().gap_1().child(
                    div()
                        .text_xs()
                        .text_color(tc(&ba, |t| t.text_tertiary))
                        .child(format!("[{source_language}]")),
                ),
            )
            .child(
                div()
                    .p_2()
                    .rounded(px(4.0))
                    .bg(tc(&ba, |t| t.background_secondary))
                    .text_sm()
                    .child(content),
            ),
        _ => {
            if content.is_empty() {
                div()
            } else {
                div().child(content)
            }
        }
    }
}
