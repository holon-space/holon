use super::prelude::*;

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text("[block_ref: no id in row]", |t| {
                    t.font_size(12).color(0xFF5252)
                });
            });
        }
    };

    match ctx.block_cache.get_or_watch(&block_id) {
        Some((render_expr, data_rows)) => {
            let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
            interpret(&render_expr, &child_ctx)
        }
        None => Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("Loading...", |t| {
                t.font_size(12).color(0x666666)
            });
        }),
    }
}
