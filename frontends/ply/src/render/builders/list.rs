use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let children: Vec<PlyWidget> = if let Some(tmpl) = template {
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
    } else {
        ctx.row()
            .iter()
            .map(|(key, value)| {
                let text = format!("{key}: {}", value.to_display_string());
                let w: PlyWidget = Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text(&text, |t| {
                        t.font_size(14).color(0xCCCCCC)
                    });
                });
                w
            })
            .collect()
    };

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(4))
            .children(|ui| {
                for child in &children {
                    child(ui);
                }
            });
    })
}
