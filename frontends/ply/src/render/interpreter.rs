use ply_engine::layout::LayoutDirection;

use holon_api::render_eval::{eval_binary_op, eval_to_value, resolve_args};
use holon_api::render_types::RenderExpr;
use holon_api::Value;

use super::context::RenderContext;

use super::builders;
use super::PlyWidget;

const MAX_QUERY_DEPTH: usize = 10;

pub fn interpret(expr: &RenderExpr, ctx: &RenderContext) -> PlyWidget {
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
            let value = ctx
                .row()
                .get(name)
                .cloned()
                .unwrap_or(Value::Null)
                .to_display_string();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&value, |t| t.font_size(14).color(0xCCCCCC));
            })
        }
        RenderExpr::Literal { value } => {
            let text = value.to_display_string();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&text, |t| t.font_size(14).color(0xCCCCCC));
            })
        }
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_to_value(left, ctx.row());
            let r = eval_to_value(right, ctx.row());
            let result = eval_binary_op(op, &l, &r).to_display_string();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.text(&result, |t| t.font_size(14).color(0xCCCCCC));
            })
        }
        RenderExpr::Array { items } => {
            let children: Vec<PlyWidget> = items.iter().map(|item| interpret(item, ctx)).collect();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.element()
                    .layout(|l| l.direction(LayoutDirection::TopToBottom))
                    .children(|ui| {
                        for child in &children {
                            child(ui);
                        }
                    });
            })
        }
        RenderExpr::Object { fields } => {
            let children: Vec<PlyWidget> = fields
                .iter()
                .map(|(_, expr)| interpret(expr, ctx))
                .collect();
            Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                ui.element()
                    .layout(|l| l.direction(LayoutDirection::TopToBottom))
                    .children(|ui| {
                        for child in &children {
                            child(ui);
                        }
                    });
            })
        }
        RenderExpr::BlockRef { block_id } => {
            if ctx.query_depth >= MAX_QUERY_DEPTH {
                let msg = format!("[block_ref recursion limit (depth {})]", ctx.query_depth);
                return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text(&msg, |t| t.font_size(12).color(0xFF5252));
                });
            }

            let block_id = block_id.clone();
            let child_ctx = ctx.deeper_query();
            match child_ctx.block_cache.get_or_watch(&block_id) {
                Some((render_expr, data_rows)) => {
                    let inner_ctx = child_ctx.with_data_rows(data_rows);
                    interpret(&render_expr, &inner_ctx)
                }
                None => Box::new(|ui: &mut ply_engine::Ui<'_, ()>| {
                    ui.text("Loading...", |t| t.font_size(12).color(0x666666));
                }),
            }
        }
    }
}
