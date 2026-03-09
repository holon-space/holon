use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let label = args
        .get_positional_string(0)
        .or(args.get_string("label"))
        .unwrap_or("")
        .to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .background_color(0x2A2A2A)
            .corner_radius(12.0)
            .layout(|l| l.padding(Padding::new(8, 8, 2, 2)))
            .children(|ui| {
                ui.text(&label, |t| {
                    t.font_size(12).color(0x888888)
                });
            });
    })
}
