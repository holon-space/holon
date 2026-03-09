use super::prelude::*;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let positional = args.get_positional_string(0);
    let title = positional
        .as_deref()
        .or(args.get_string("title"))
        .unwrap_or("Section")
        .to_string();

    let children: Vec<PlyWidget> = if let Some(tmpl) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
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
        vec![]
    };

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .background_color(0x1E1E1E)
            .corner_radius(8.0)
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(8).padding(16u16))
            .children(|ui| {
                ui.text(&title, |t| {
                    t.font_size(18).color(0xE0E0E0)
                });
                for child in &children {
                    child(ui);
                }
            });
    })
}
