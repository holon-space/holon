use holon_api::EntityName;
use holon_api::render_eval::resolve_args;
use holon_api::render_types::Arg;
use holon_api::render_types::ClickModifiers;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::render_types::Trigger;
use holon_api::widget_spec::DataRow;

use super::prelude::*;

fn build_wiring(name: &str, args: &[Arg], row: &DataRow, trigger: Trigger) -> OperationWiring {
    let (entity_name, op_name) = match name.split_once('.') {
        Some((e, o)) => (e.to_string(), o.to_string()),
        None => ("block".to_string(), name.to_string()),
    };
    let resolved = resolve_args(args, row);
    let mut bound_params: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    for (k, v) in &resolved.named {
        bound_params.insert(k.clone(), v.clone());
    }
    for (i, v) in resolved.positional.iter().enumerate() {
        bound_params.insert(format!("pos_{i}"), v.clone());
    }
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: EntityName::new(entity_name),
            name: op_name,
            trigger: Some(trigger),
            bound_params,
            entity_short_name: String::new(),
            id_column: "id".to_string(),
            display_name: String::new(),
            description: String::new(),
            required_params: vec![],
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::PointerGesture,
            },
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

holon_macros::widget_builder! {
    fn selectable(
        action: Expr,
        shift_action: Expr,
        cmd_action: Expr,
        ctrl_action: Expr,
        alt_action: Expr,
    ) -> ViewModel {
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
        if let Some(RenderExpr::FunctionCall { name, args, .. }) = action {
            operations.push(build_wiring(
                name,
                args,
                ba.ctx.row(),
                Trigger::Click {
                    modifiers: ClickModifiers::none(),
                },
            ));
        }
        // Modifier-bound secondary actions. Each `<modifier>_action:` template
        // declares a SEPARATE wiring fired only when that modifier is held at
        // click — a fully declarative "secondary action for modifier-click".
        // Frontends `stop_propagation` on any modifier-click path so the
        // row-level click doesn't also fire. This is the generic capability;
        // tabs are its first user (Q2): the left sidebar declares
        // `cmd_action: navigation_open_tab(...)` (macOS) and
        // `ctrl_action: navigation_open_tab(...)` (Windows/Linux) so a
        // modifier-click opens the page in a new tab instead of replacing the
        // current one. Adding a modifier is one table row here plus its name in
        // `holon_api::render_eval::is_template_arg` — without the latter the arg
        // parses as a scalar and `get_template` silently returns `None`, leaving
        // the affordance dead. No change to the `Trigger` enum or the frontend
        // dispatch (it looks the active modifier set up in a
        // `HashMap<ClickModifiers, _>`).
        // Touch mapping (mobile): long-press → the same secondary action; the
        // desktop-first wiring above is the reference, mobile long-press routes
        // to the identical `<modifier>_action` intent.
        for (template, modifiers) in [
            (shift_action, ClickModifiers::shift()),
            (cmd_action, ClickModifiers::cmd()),
            (ctrl_action, ClickModifiers::ctrl()),
            (alt_action, ClickModifiers::alt()),
        ] {
            if let Some(RenderExpr::FunctionCall { name, args, .. }) = template {
                operations.push(build_wiring(name, args, ba.ctx.row(), Trigger::Click {
                    modifiers,
                }));
            }
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
