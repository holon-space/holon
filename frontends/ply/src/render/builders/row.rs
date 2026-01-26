use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let mut children: Vec<PlyWidget> = Vec::new();

    if let Some(template) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        children.push(interpret(template, ctx));
    }

    for val in &args.positional {
        if let holon_api::Value::String(s) = val {
            let s = s.clone();
            children.push(Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&s, |t| {
                    t.font_size(14).color(0xCCCCCC)
                });
            }));
        }
    }

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::LeftToRight).gap(8))
            .children(|ui| {
                for child in &children {
                    child(ui);
                }
            });
    })
}
