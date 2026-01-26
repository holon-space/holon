use std::collections::HashMap;

use holon_api::render_types::{Arg, BinaryOperator, RenderExpr};
use holon_api::{is_template_arg, Value};
use waterui::prelude::*;

use super::builders;
use super::context::RenderContext;

pub fn interpret(expr: &RenderExpr, ctx: &RenderContext) -> AnyView {
    match expr {
        RenderExpr::FunctionCall {
            name,
            args,
            operations,
        } => {
            let resolved = resolve_args(args, ctx);
            let child_ctx = RenderContext {
                data_rows: ctx.data_rows.clone(),
                operations: operations.clone(),
                session: ctx.session.clone(),
                runtime_handle: ctx.runtime_handle.clone(),
                depth: ctx.depth,
                query_depth: ctx.query_depth,
            };
            builders::build(name, &resolved, &child_ctx)
        }
        RenderExpr::ColumnRef { name } => {
            let value = ctx.row().get(name).cloned().unwrap_or(Value::Null);
            AnyView::new(text(value_to_string(&value)).size(14.0))
        }
        RenderExpr::Literal { value } => AnyView::new(text(value_to_string(value)).size(14.0)),
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_to_value(left, ctx);
            let r = eval_to_value(right, ctx);
            let result = eval_binary_op(op, &l, &r);
            AnyView::new(text(value_to_string(&result)).size(14.0))
        }
        RenderExpr::Array { items } => {
            let views: Vec<AnyView> = items.iter().map(|item| interpret(item, ctx)).collect();
            AnyView::new(vstack(views))
        }
        RenderExpr::Object { fields } => {
            let views: Vec<AnyView> = fields
                .iter()
                .map(|(_, expr)| interpret(expr, ctx))
                .collect();
            AnyView::new(vstack(views))
        }
        RenderExpr::BlockRef { block_id } => build_block_ref(block_id, ctx),
    }
}

pub struct ResolvedArgs {
    pub positional: Vec<Value>,
    pub positional_exprs: Vec<RenderExpr>,
    pub named: HashMap<String, Value>,
    pub templates: HashMap<String, RenderExpr>,
}

impl ResolvedArgs {
    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.named.get(name).and_then(|v| v.as_string())
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.named.get(name).and_then(|v| match v {
            Value::Float(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        })
    }

    #[allow(dead_code)]
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.named.get(name).and_then(|v| match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        })
    }

    pub fn get_positional_string(&self, index: usize) -> Option<&str> {
        <[Value]>::get(&self.positional, index).and_then(|v| v.as_string())
    }

    pub fn get_template(&self, name: &str) -> Option<&RenderExpr> {
        self.templates.get(name)
    }
}

fn resolve_args(args: &[Arg], ctx: &RenderContext) -> ResolvedArgs {
    let mut positional = Vec::new();
    let mut positional_exprs = Vec::new();
    let mut named = HashMap::new();
    let mut templates = HashMap::new();

    for arg in args {
        match &arg.name {
            Some(name) if is_template_arg(name) => {
                templates.insert(name.clone(), arg.value.clone());
            }
            Some(name) => {
                named.insert(name.clone(), eval_to_value(&arg.value, ctx));
            }
            None => {
                if let RenderExpr::ColumnRef { name: col_name } = &arg.value {
                    named.insert(
                        format!("_pos_{}_field", positional.len()),
                        Value::String(col_name.clone()),
                    );
                }
                positional_exprs.push(arg.value.clone());
                positional.push(eval_to_value(&arg.value, ctx));
            }
        }
    }

    ResolvedArgs {
        positional,
        positional_exprs,
        named,
        templates,
    }
}

pub fn eval_to_value(expr: &RenderExpr, ctx: &RenderContext) -> Value {
    match expr {
        RenderExpr::Literal { value } => value.clone(),
        RenderExpr::ColumnRef { name } => ctx.row().get(name).cloned().unwrap_or(Value::Null),
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_to_value(left, ctx);
            let r = eval_to_value(right, ctx);
            eval_binary_op(op, &l, &r)
        }
        RenderExpr::FunctionCall { name, args, .. } => match name.as_str() {
            "concat" => {
                let resolved = resolve_args(args, ctx);
                let parts: Vec<String> = resolved.positional.iter().map(value_to_string).collect();
                Value::String(parts.join(""))
            }
            _ => {
                if let Some(first) = args.first() {
                    eval_to_value(&first.value, ctx)
                } else {
                    Value::Null
                }
            }
        },
        RenderExpr::Array { items } => {
            Value::Array(items.iter().map(|i| eval_to_value(i, ctx)).collect())
        }
        RenderExpr::Object { fields } => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), eval_to_value(v, ctx)))
                .collect(),
        ),
        RenderExpr::BlockRef { block_id } => Value::String(format!("[BlockRef: {}]", block_id)),
    }
}

fn eval_binary_op(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
    match op {
        BinaryOperator::Add => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a + b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
            (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
            _ => Value::Null,
        },
        BinaryOperator::Sub => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a - b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a - b),
            _ => Value::Null,
        },
        BinaryOperator::Mul => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a * b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a * b),
            _ => Value::Null,
        },
        BinaryOperator::Div => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) if *b != 0 => Value::Integer(a / b),
            (Value::Float(a), Value::Float(b)) if *b != 0.0 => Value::Float(a / b),
            _ => Value::Null,
        },
        BinaryOperator::Eq => Value::Boolean(left == right),
        BinaryOperator::Neq => Value::Boolean(left != right),
        BinaryOperator::Gt => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a > b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a > b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Lt => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a < b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a < b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Gte => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a >= b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a >= b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Lte => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a <= b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a <= b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::And => match (left, right) {
            (Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a && *b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Or => match (left, right) {
            (Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a || *b),
            _ => Value::Boolean(false),
        },
    }
}

fn build_block_ref(block_id: &str, ctx: &RenderContext) -> AnyView {
    let deeper = ctx.deeper_query();
    let session = ctx.session.clone();
    let handle = ctx.runtime_handle.clone();
    let bid = block_id.to_string();

    let result = std::thread::scope(|s| {
        s.spawn(|| handle.block_on(session.engine().blocks().render_block(&bid, None, false)))
            .join()
            .unwrap()
    });

    match result {
        Ok((widget_spec, _stream)) => {
            let child_ctx = deeper.with_data_rows(widget_spec.data.clone());
            interpret(&widget_spec.render_expr, &child_ctx)
        }
        Err(e) => {
            tracing::warn!("render_block({bid}) failed: {e}");
            AnyView::new(
                text(format!("render_block error: {e}"))
                    .size(12.0)
                    .foreground(Color::srgb_hex("#FF0000")),
            )
        }
    }
}

pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::DateTime(dt) => dt.clone(),
        Value::Null => String::new(),
        Value::Json(j) => j.clone(),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(value_to_string).collect();
            parts.join(", ")
        }
        Value::Object(map) => serde_json::to_string(map).unwrap_or_default(),
    }
}
