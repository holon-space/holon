use super::prelude::*;

use crate::render::interpreter;

/// block_ref() builder — renders a nested block using a cached watch_ui stream.
///
/// On first encounter, starts a watch_ui stream for the block and caches the
/// result. On subsequent frames, reads from cache without re-querying.
pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            return div().child(
                text("[block_ref: no id in row]")
                    .size(12.0)
                    .color(theme.color(ColorToken::Error)),
            );
        }
    };

    match ctx.block_cache.get_or_watch(&block_id) {
        Some((render_expr, data_rows)) => {
            let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
            interpreter::interpret(&render_expr, &child_ctx)
        }
        None => div().child(
            text("Loading...")
                .size(12.0)
                .color(theme.color(ColorToken::TextTertiary)),
        ),
    }
}
