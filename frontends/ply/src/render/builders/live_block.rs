use super::prelude::*;

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let block_uri = match ctx.row().get("id").and_then(|v| v.as_string()) {
        Some(id) => holon_api::EntityUri::parse(id)
            .expect("live_block row id is not a valid EntityUri"),
        None => {
            return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text("[live_block: no id in row]", |t| {
                    t.font_size(12).color(0xFF5252)
                });
            });
        }
    };

    let (render_expr, data_rows) = ctx.get_block_data(&block_uri);
    let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
    interpret(&render_expr, &child_ctx)
}
