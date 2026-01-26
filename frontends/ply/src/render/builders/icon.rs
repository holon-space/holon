use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let positional = args.get_positional_string(0);
    let name = positional
        .as_deref()
        .or(args.get_string("name"))
        .unwrap_or("circle")
        .to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&format!("[{name}]"), |t| {
            t.font_size(14).color(0xCCCCCC)
        });
    })
}
