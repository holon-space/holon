use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let positional = args.get_positional_string(0);
    let message = args
        .get_string("message")
        .or(positional.as_deref())
        .unwrap_or("Unknown error")
        .to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .background_color(0x3C1518)
            .corner_radius(4.0)
            .layout(|l| l.padding(8u16))
            .children(|ui| {
                ui.text(&message, |t| {
                    t.font_size(12).color(0xFF5252)
                });
            });
    })
}
