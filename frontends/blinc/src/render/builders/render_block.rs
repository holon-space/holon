use super::prelude::*;

use crate::render::interpreter;

/// render_block(this) — dispatches based on block content_type.
///
/// - content_type: "source" + source_language is a query language → cached watch_ui
/// - content_type: "source" → source code display
/// - default → show content as text
pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let content_type = ctx
        .row()
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("");
    let source_language = ctx
        .row()
        .get("source_language")
        .and_then(|v| v.as_string())
        .unwrap_or("");
    let content = ctx
        .row()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("");

    let is_query_lang = source_language.parse::<holon_api::QueryLanguage>().is_ok();
    let theme = ThemeState::get();
    match (content_type, is_query_lang) {
        ("source", true) => {
            let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
                Some(id) => id.to_string(),
                None => {
                    return div().child(
                        text("[render_block: no id]")
                            .size(12.0)
                            .color(theme.color(ColorToken::Error)),
                    );
                }
            };

            let (render_expr, data_rows) = ctx.block_watch().get_or_watch(&block_id);
            let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
            interpreter::interpret(&render_expr, &child_ctx)
        }
        ("source", false) => div()
            .flex_col()
            .gap(2.0)
            .child(
                div().flex_row().gap(4.0).child(
                    text(format!("[{source_language}]"))
                        .size(10.0)
                        .color(theme.color(ColorToken::TextTertiary)),
                ),
            )
            .child(
                div()
                    .p(8.0)
                    .rounded(4.0)
                    .bg(theme.color(ColorToken::SurfaceOverlay))
                    .child(
                        text(content)
                            .size(13.0)
                            .color(theme.color(ColorToken::TextPrimary)),
                    ),
            ),
        _ => {
            if let Some(profile) = ctx.session().resolve_row_profile(ctx.row()) {
                interpreter::interpret(&profile.render, ctx)
            } else if content.is_empty() {
                div()
            } else {
                div().child(
                    text(content)
                        .size(14.0)
                        .color(theme.color(ColorToken::TextPrimary)),
                )
            }
        }
    }
}
