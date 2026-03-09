use super::prelude::*;
use holon_api::render_eval::{column_ref_name, OutlineTree};

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let parent_id_col = args
        .get_template("parent_id")
        .and_then(column_ref_name)
        .unwrap_or("parent_id");
    let sort_col = args
        .get_template("sortkey")
        .or(args.get_template("sort_key"))
        .and_then(column_ref_name)
        .unwrap_or("sort_key");

    let Some(tmpl) = template else {
        return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("[outline: no item_template]", |t| {
                t.font_size(12).color(0x888888)
            });
        });
    };

    let rows = &ctx.data_rows;
    if rows.is_empty() {
        return interpret(tmpl, ctx);
    }

    let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col);
    let elements: Vec<(PlyWidget, usize)> = tree.walk_depth_first(|row, depth| {
        let row_ctx = ctx.with_row(row.clone());
        let row_ctx = RenderContext {
            depth: row_ctx.depth + depth,
            ..row_ctx
        };
        (interpret(tmpl, &row_ctx), depth)
    });

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(2))
            .children(|ui| {
                for (widget, depth) in &elements {
                    let indent = (*depth as u16) * 16;
                    ui.element()
                        .layout(|l| {
                            l.direction(LayoutDirection::LeftToRight)
                                .padding(Padding::new(indent, 0, 0, 0))
                        })
                        .children(|ui| {
                            widget(ui);
                        });
                }
            });
    })
}
