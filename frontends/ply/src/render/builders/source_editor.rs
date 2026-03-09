use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> PlyWidget {
    let language = args.get_string("language").unwrap_or("text").to_string();
    let content = args.get_string("content").unwrap_or("").to_string();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(4))
            .children(|ui| {
                ui.text(&language, |t| {
                    t.font_size(11).color(0x888888)
                });
                ui.element()
                    .background_color(0x2A2A2A)
                    .corner_radius(4.0)
                    .layout(|l| l.padding(8u16))
                    .children(|ui| {
                        ui.text(&content, |t| {
                            t.font_size(12).color(0xCCCCCC)
                        });
                    });
            });
    })
}
