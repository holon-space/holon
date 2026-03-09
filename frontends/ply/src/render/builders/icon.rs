use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let name = args
        .get_positional_string(0)
        .or(args.get_string("name"))
        .unwrap_or("circle")
        .to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&format!("[{name}]"), |t| {
            t.font_size(14).color(0xCCCCCC)
        });
    })
}
