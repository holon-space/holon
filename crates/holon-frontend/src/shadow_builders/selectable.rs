use super::prelude::*;
use holon_api::render_eval::resolve_args;
use holon_api::render_types::{
    OperationDescriptor, OperationWiring, RenderExpr, Trigger,
};
use holon_api::EntityName;

holon_macros::widget_builder! {
    raw fn selectable(ba: BA<'_>) -> ViewModel {
        let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
            (ba.interpret)(child_expr, ba.ctx)
        } else {
            ViewModel::empty()
        };

        // Parse the optional `action:` arg into a click-triggered operation
        // wiring. The DSL form `navigation_focus(#{...})` is rewritten to dot-form
        // (`navigation.focus`) by Rhai aliasing at parse time, so `name` here is
        // already `entity.op`.
        //
        // Args are resolved against the current row at interpret time and stored
        // as `bound_params`. Positional args are stashed under `pos_<i>` to
        // preserve the previous wire format for ops that consume them.
        let mut operations = Vec::new();
        if let Some(RenderExpr::FunctionCall { name, args, .. }) = ba.args.get_template("action") {
            let (entity_name, op_name) = match name.split_once('.') {
                Some((e, o)) => (e.to_string(), o.to_string()),
                None => ("block".to_string(), name.clone()),
            };
            let resolved = resolve_args(args, ba.ctx.row());
            let mut bound_params: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
            for (k, v) in &resolved.named {
                bound_params.insert(k.clone(), v.clone());
            }
            for (i, v) in resolved.positional.iter().enumerate() {
                bound_params.insert(format!("pos_{i}"), v.clone());
            }
            operations.push(OperationWiring {
                modified_param: String::new(),
                descriptor: OperationDescriptor {
                    entity_name: EntityName::new(entity_name),
                    name: op_name,
                    trigger: Some(Trigger::Click),
                    bound_params,
                    ..Default::default()
                },
            });
        }

        // Wire to the shared per-row signal cell. `bound_params` here are
        // resolved from `col(...)` references at build time and baked into
        // the click `OperationWiring` — they cover entity ids and similar
        // primary-key columns that don't change across CDC updates. If a
        // future caller resolves bound_params from a frequently-mutating
        // column (e.g. `content`), we'll need to either make `operations`
        // a `Mutable` or re-resolve at click time using `data.get_cloned()`
        // — defer that until the use case appears.
        ViewModel {
            data: ba.ctx.data_mutable(),
            operations,
            children: vec![Arc::new(child)],
            ..ViewModel::from_widget("selectable", std::collections::HashMap::new())
        }
    }
}
