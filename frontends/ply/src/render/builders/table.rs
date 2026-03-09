use super::prelude::*;

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    if ctx.data_rows.is_empty() {
        return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("[empty]", |t| {
                t.font_size(12).color(0x888888)
            });
        });
    }

    let mut columns: Vec<String> = ctx.data_rows[0].keys().cloned().collect();
    columns.sort();

    let rows: Vec<Vec<String>> = ctx
        .data_rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|col| {
                    row.get(col)
                        .map(|v| v.to_display_string())
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();

    Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
        ui.element()
            .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(2))
            .children(|ui| {
                // Header
                ui.element()
                    .layout(|l| l.direction(LayoutDirection::LeftToRight).gap(8))
                    .children(|ui| {
                        for col in &columns {
                            ui.element().width(fixed(120.0)).children(|ui| {
                                ui.text(col, |t| {
                                    t.font_size(11).color(0x888888)
                                });
                            });
                        }
                    });
                // Data rows
                for row in &rows {
                    ui.element()
                        .layout(|l| l.direction(LayoutDirection::LeftToRight).gap(8))
                        .children(|ui| {
                            for val in row {
                                ui.element().width(fixed(120.0)).children(|ui| {
                                    ui.text(val, |t| {
                                        t.font_size(12).color(0xCCCCCC)
                                    });
                                });
                            }
                        });
                }
            });
    })
}
