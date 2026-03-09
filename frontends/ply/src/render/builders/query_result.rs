use super::prelude::*;
use holon_api::Value;

pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let result_value = ctx
        .row()
        .get("result")
        .or_else(|| ctx.row().get("results"))
        .cloned()
        .unwrap_or(Value::Null);

    match &result_value {
        Value::Null => Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("[no result]", |t| {
                t.font_size(12).color(0x888888)
            });
        }),
        Value::String(s) if s.is_empty() => Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
            ui.text("[empty result]", |t| {
                t.font_size(12).color(0x888888)
            });
        }),
        Value::String(s) => {
            let s = s.clone();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&s, |t| {
                    t.font_size(12).color(0xCCCCCC)
                });
            })
        }
        other => {
            let text = format!("{other:?}");
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&text, |t| {
                    t.font_size(12).color(0xCCCCCC)
                });
            })
        }
    }
}
