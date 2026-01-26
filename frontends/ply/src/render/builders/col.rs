use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let children: Vec<PlyWidget> = args
        .positional_exprs
        .iter()
        .map(|expr| interpret(expr, ctx))
        .collect();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom))
            .children(|ui| {
                for child in &children {
                    child(ui);
                }
            });
    })
}
