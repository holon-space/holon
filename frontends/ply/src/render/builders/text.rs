use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let content = args
        .get_positional_string(0)
        .map(|s| s.to_string())
        .or_else(|| args.get_string("content").map(|s| s.to_string()))
        .unwrap_or_else(|| {
            args.positional
                .first()
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        });

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.text(&content, |t| {
            t.font_size(14).color(0xCCCCCC)
        });
    })
}
