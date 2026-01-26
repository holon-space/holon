use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let checked = args.get_bool("checked").unwrap_or(false);
    let symbol = if checked { "[x]" } else { "[ ]" };
    let color: u32 = if checked { 0x4CAF50 } else { 0x888888 };

    let symbol = symbol.to_string();
    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&symbol, |t| {
            t.font_size(14).color(color)
        });
    })
}
