use super::prelude::*;

use crate::render::interpreter::interpret;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let gap = args.get_f64("gap").unwrap_or(4.0) as f32;
    let template = args
        .get_template("item_template")
        .or(args.get_template("item"));

    let mut container = div().flex_col().gap(gap);

    if let Some(tmpl) = template {
        if ctx.data_rows.is_empty() {
            container = container.child(interpret(tmpl, ctx));
        } else {
            for row in &ctx.data_rows {
                let row_ctx = ctx.with_row(row.clone());
                container = container.child(interpret(tmpl, &row_ctx));
            }
        }
    } else {
        for (key, value) in ctx.row() {
            container = container.child(
                text(format!("{key}: {}", value.to_display_string()))
                    .size(13.0)
                    .color(ThemeState::get().color(ColorToken::TextSecondary)),
            );
        }
    }

    container
}
