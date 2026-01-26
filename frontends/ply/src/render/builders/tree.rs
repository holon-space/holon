use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let children: Vec<PlyWidget> = match template {
        Some(tmpl) => {
            if ctx.data_rows.is_empty() {
                vec![interpret(tmpl, ctx)]
            } else {
                ctx.data_rows
                    .iter()
                    .map(|row| {
                        let row_ctx = ctx.with_row(row.clone());
                        interpret(tmpl, &row_ctx)
                    })
                    .collect()
            }
        }
        None => {
            let w: PlyWidget = Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text("[tree: no item_template]", |t| {
                    t.font_size(12).color(0x888888)
                });
            });
            vec![w]
        }
    };

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(2))
            .children(|ui| {
                for child in &children {
                    child(ui);
                }
            });
    })
}
