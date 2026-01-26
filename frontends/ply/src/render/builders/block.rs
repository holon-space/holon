use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let indent = (ctx.depth as u16) * 29;

    let child = if let Some(template) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        interpret(template, ctx)
    } else {
        empty_widget()
    };

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).padding(Padding::new(indent, 0, 0, 0)))
            .children(|ui| {
                child(ui);
            });
    })
}
