use super::prelude::*;
use holon_api::render_eval::{has_drawer_rows, partition_screen_columns, sort_key_column, sorted_rows};
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;

const SIDEBAR_WIDTH: f32 = 280.0;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let tmpl = match template {
        Some(t) => t,
        None => return empty_widget(),
    };

    let rows = sorted_rows(&ctx.data_rows, sort_key_column(args));

    if has_drawer_rows(&rows) {
        return build_screen_layout(tmpl, &rows, ctx);
    }

    let children: Vec<PlyWidget> = if rows.is_empty() {
        vec![interpret(tmpl, ctx)]
    } else {
        rows.iter()
            .map(|row| {
                let row_ctx = ctx.with_row(row.clone());
                interpret(tmpl, &row_ctx)
            })
            .collect()
    };

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .width(grow!())
            .layout(|l| l.direction(LayoutDirection::LeftToRight).gap(16))
            .children(|ui| {
                for child in &children {
                    ui.element()
                        .width(grow!())
                        .layout(|l| l.direction(LayoutDirection::TopToBottom))
                        .children(|ui| {
                            child(ui);
                        });
                }
            });
    })
}

fn build_screen_layout(tmpl: &RenderExpr, rows: &[DataRow], ctx: &RenderContext) -> PlyWidget {
    if rows.is_empty() {
        let child_ctx = ctx.with_row(Default::default());
        return interpret(tmpl, &child_ctx);
    }

    let partition = partition_screen_columns(rows, |row| {
        let row_ctx = ctx.with_row(row.clone());
        interpret(tmpl, &row_ctx)
    });

    let main_widgets: Vec<PlyWidget> = partition.main.into_iter().map(|r| r.widget).collect();
    let left_sidebar = partition.left_sidebar;
    let right_sidebar = partition.right_sidebar;

    if let Some(ref region) = left_sidebar {
        if let Some(ref bid) = region.block_id {
            *ctx.ext.left_sidebar_block_id.lock().unwrap() = Some(bid.clone());
        }
    }

    let ws = &ctx.widget_states();
    let left_open = left_sidebar.as_ref().map(|r| {
        let key = r.block_id.as_deref().unwrap_or("");
        (
            ws.get(key).map(|s| s.open).unwrap_or(true),
            ws.get(key).and_then(|s| s.width).unwrap_or(SIDEBAR_WIDTH),
        )
    });
    let right_open = right_sidebar.as_ref().map(|r| {
        let key = r.block_id.as_deref().unwrap_or("");
        (
            ws.get(key).map(|s| s.open).unwrap_or(true),
            ws.get(key).and_then(|s| s.width).unwrap_or(SIDEBAR_WIDTH),
        )
    });

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .width(grow!())
            .height(grow!())
            .layout(|l| l.direction(LayoutDirection::LeftToRight))
            .children(|ui| {
                if let (Some(ref region), Some((is_open, width))) = (&left_sidebar, left_open) {
                    if is_open {
                        ui.element()
                            .width(fixed(width))
                            .height(grow!())
                            .background_color(0x1E1E1E)
                            .layout(|l| l.direction(LayoutDirection::TopToBottom))
                            .children(|ui| {
                                (region.widget)(ui);
                            });
                    }
                }

                if main_widgets.len() == 1 {
                    ui.element()
                        .width(grow!())
                        .height(grow!())
                        .layout(|l| l.direction(LayoutDirection::TopToBottom))
                        .children(|ui| {
                            main_widgets[0](ui);
                        });
                } else {
                    ui.element()
                        .width(grow!())
                        .height(grow!())
                        .layout(|l| l.direction(LayoutDirection::LeftToRight))
                        .children(|ui| {
                            for widget in &main_widgets {
                                ui.element()
                                    .width(grow!())
                                    .height(grow!())
                                    .layout(|l| l.direction(LayoutDirection::TopToBottom))
                                    .children(|ui| {
                                        widget(ui);
                                    });
                            }
                        });
                }

                if let (Some(ref region), Some((is_open, width))) = (&right_sidebar, right_open) {
                    if is_open {
                        ui.element()
                            .width(fixed(width))
                            .height(grow!())
                            .background_color(0x1E1E1E)
                            .layout(|l| l.direction(LayoutDirection::TopToBottom))
                            .children(|ui| {
                                (region.widget)(ui);
                            });
                    }
                }
            });
    })
}
