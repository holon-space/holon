use super::prelude::*;

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let content_type = ctx
        .row()
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let source_language = ctx
        .row()
        .get("source_language")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let content = ctx
        .row()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let is_query_lang = source_language.parse::<holon_api::QueryLanguage>().is_ok();

    match (content_type.as_str(), is_query_lang) {
        ("source", true) => {
            let block_id = match ctx.row().get("id").and_then(|v| v.as_string()) {
                Some(id) => id.to_string(),
                None => {
                    return Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                        ui.text("[render_block: no id]", |t| {
                            t.font_size(12).color(0xFF5252)
                        });
                    });
                }
            };

            match ctx.block_cache.get_or_watch(&block_id) {
                Some((render_expr, data_rows)) => {
                    let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
                    interpret(&render_expr, &child_ctx)
                }
                None => Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text("Loading...", |t| {
                        t.font_size(12).color(0x666666)
                    });
                }),
            }
        }
        ("source", false) => Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            ui.element()
                .layout(|l| l.direction(LayoutDirection::TopToBottom).gap(4))
                .children(|ui| {
                    ui.text(&format!("[{source_language}]"), |t| {
                        t.font_size(11).color(0x666666)
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
        }),
        _ => {
            if content.is_empty() {
                empty_widget()
            } else {
                Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text(&content, |t| {
                        t.font_size(14).color(0xCCCCCC)
                    });
                })
            }
        }
    }
}
