use holon_api::EntityName;
use holon_api::input_types::Key;
use holon_api::input_types::KeyChord;
use holon_api::render_eval::resolve_args;
use holon_api::render_types::Arg;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::render_types::Trigger;
use holon_api::widget_spec::DataRow;

use super::prelude::*;

/// Build the submit wiring from the `action:` template. The dot-form
/// `entity.op` name and the row-resolved `bound_params` follow `selectable`;
/// what differs is `modified_param` — the one operation parameter the widget
/// itself fills, at dispatch, from the draft buffer.
fn submit_wiring(
    name: &str,
    args: &[Arg],
    row: &DataRow,
    modified_param: String,
    id_param: String,
) -> OperationWiring {
    let (entity_name, op_name) = match name.split_once('.') {
        Some((e, o)) => (e.to_string(), o.to_string()),
        None => ("block".to_string(), name.to_string()),
    };
    let resolved = resolve_args(args, row);
    let mut bound_params: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    // The row's `id` names the entity this box composes for. Cache tables store
    // it scheme-qualified (`cc-session:<id>`) while the operation wants the
    // bare id, so the URI is unwrapped here rather than at every declaration
    // site. A box with no row is not bound to an entity and binds nothing.
    if let Some(raw) = row.get("id").and_then(|v| v.as_string()) {
        let target = holon_api::EntityUri::parse(raw)
            .unwrap_or_else(|e| panic!("input_box: row id must be an entity URI: {e}"));
        bound_params.insert(id_param, Value::String(target.id().to_string()));
    }
    for (k, v) in &resolved.named {
        bound_params.insert(k.clone(), v.clone());
    }
    for (i, v) in resolved.positional.iter().enumerate() {
        bound_params.insert(format!("pos_{i}"), v.clone());
    }
    OperationWiring {
        modified_param,
        descriptor: OperationDescriptor {
            entity_name: EntityName::new(entity_name),
            name: op_name,
            trigger: Some(Trigger::KeyChord {
                chord: KeyChord::new(&[Key::Enter]),
            }),
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
                surface: holon_api::NonMenuSurface::KeyboardGesture,
            },
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

holon_macros::widget_builder! {
    fn input_box(
        #[default = ""] placeholder: String,
        action: Expr,
        #[default = false] multiline: bool,
        #[default = "Send"] submit_label: String,
        #[default = "content"] text_param: String,
        #[default = "id"] id_param: String,
    ) -> ViewModel {
        // `action:` is the whole point of the widget — a compose box with no
        // wired operation has nothing to submit to, so say so on screen rather
        // than render a box that silently eats what the user types.
        let Some(RenderExpr::FunctionCall { name, args, .. }) = action else {
            return ViewModel::error(
                "input_box",
                "input_box requires an `action:` operation template".to_string(),
            );
        };
        let operations = vec![submit_wiring(name, args, ba.ctx.row(), text_param, id_param)];

        let mut __props = std::collections::HashMap::new();
        __props.insert("placeholder".to_string(), Value::String(placeholder));
        __props.insert("multiline".to_string(), Value::Boolean(multiline));
        __props.insert("submit_label".to_string(), Value::String(submit_label));

        // The draft tracks NO row. `editable_text` re-derives its content from
        // a column on every CDC write; a compose buffer must survive exactly
        // that churn, so its authority is this node-owned `Mutable` and the
        // only thing that empties it is a successful dispatch.
        ViewModel {
            draft: Some(futures_signals::signal::Mutable::new(String::new())),
            send_state: Some(futures_signals::signal::Mutable::new(
                crate::reactive_view_model::SendState::Idle,
            )),
            // The empty draft this node starts with IS a submission slot, so
            // it gets its identity now; the next one is minted when a proven
            // delivery empties the box again.
            compose_id: Some(futures_signals::signal::Mutable::new(
                holon_api::effect_id::ComposeId::mint(),
            )),
            data: ba.ctx.data_mutable(),
            operations,
            ..ViewModel::from_widget("input_box", __props)
        }
    }
}
