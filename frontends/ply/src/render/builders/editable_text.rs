use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let content = args
        .get_positional_string(0)
        .or_else(|| args.get_string("content"))
        .unwrap_or("")
        .to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&content, |t| {
            t.font_size(14).color(0xCCCCCC)
        });
    })
}
