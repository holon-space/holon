use blinc_app::prelude::*;
use blinc_theme::{ColorToken, ThemeState};
use holon_api::render_eval::{eval_to_value, resolve_args};
use holon_api::render_types::RenderExpr;
use holon_api::Value;

use super::builders;
use super::context::RenderContext;

pub fn interpret(expr: &RenderExpr, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    match expr {
        RenderExpr::FunctionCall {
            name,
            args,
            operations,
        } => {
            let resolved = resolve_args(args, ctx.row());
            let child_ctx = ctx.with_operations(operations.clone());
            builders::build(name, &resolved, &child_ctx)
        }
        RenderExpr::ColumnRef { name } => {
            let value = ctx.row().get(name).cloned().unwrap_or(Value::Null);
            div().child(
                text(value.to_display_string())
                    .size(14.0)
                    .color(theme.color(ColorToken::TextPrimary)),
            )
        }
        RenderExpr::Literal { value } => div().child(
            text(value.to_display_string())
                .size(14.0)
                .color(theme.color(ColorToken::TextPrimary)),
        ),
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_to_value(left, ctx.row());
            let r = eval_to_value(right, ctx.row());
            let result = holon_api::render_eval::eval_binary_op(op, &l, &r);
            div().child(
                text(result.to_display_string())
                    .size(14.0)
                    .color(theme.color(ColorToken::TextPrimary)),
            )
        }
        RenderExpr::Array { items } => {
            let mut container = div().flex_col();
            for item in items {
                container = container.child(interpret(item, ctx));
            }
            container
        }
        RenderExpr::Object { fields } => {
            let mut container = div().flex_col();
            for (_key, expr) in fields {
                container = container.child(interpret(expr, ctx));
            }
            container
        }
        RenderExpr::BlockRef { block_id } => build_block_ref(block_id, ctx),
    }
}

fn build_block_ref(block_id: &str, ctx: &RenderContext) -> Div {
    let (render_expr, data_rows) = ctx.block_watch().get_or_watch(block_id);
    let child_ctx = ctx.deeper_query().with_data_rows(data_rows);
    interpret(&render_expr, &child_ctx)
}
