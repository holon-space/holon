use super::prelude::*;
use crate::view_model::NodeKind;
use holon_api::render_eval::resolve_args;
use holon_api::render_types::RenderExpr;

holon_macros::widget_builder! {
    raw fn selectable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };

        let mut entity = ba.ctx.row().clone();

        if let Some(RenderExpr::FunctionCall { name, args, .. }) = ba.args.get_template("action") {
            entity.insert("__action_name".to_string(), Value::String(name.clone()));
            let resolved = resolve_args(args, ba.ctx.row());
            for (k, v) in &resolved.named {
                entity.insert(format!("__action_{k}"), v.clone());
            }
            for (i, v) in resolved.positional.iter().enumerate() {
                entity.insert(format!("__action_pos_{i}"), v.clone());
            }
        }

        ViewModel {
            entity,
            kind: NodeKind::Selectable {
                child: Box::new(child),
            },
            ..Default::default()
        }
    }
}
