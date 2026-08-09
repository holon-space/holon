use holon_api::EntityName;
use holon_api::render_eval::resolve_args;
use holon_api::render_types::ClickModifiers;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::render_types::Trigger;
use holon_api::widget_spec::DataRow;

use super::prelude::*;

/// One answer the question actually offers. Parsed out of the `options` JSON
/// column so the rest of the widget cannot invent a label the question never
/// offered — that set is the whole safety property.
struct OfferedAnswer {
    label: String,
}

fn parse_offered(raw: &str) -> anyhow::Result<Vec<OfferedAnswer>> {
    let parsed: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("`options` is not JSON ({e}): {raw}"))?;
    let array = parsed
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("`options` must be a JSON array, got: {raw}"))?;
    array
        .iter()
        .map(|option| {
            let label = option
                .get("label")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!("every `options` entry needs a string `label`, got: {option}")
                })?;
            Ok(OfferedAnswer {
                label: label.to_string(),
            })
        })
        .collect()
}

/// A click-triggered wiring answering the question with exactly `label`.
///
/// The provider takes the chosen labels as an ARRAY — several entries express a
/// multi-select. One button is one selection, so it dispatches a one-element
/// array; a scalar is refused by the tool.
fn answer_wiring(
    entity_name: &str,
    op_name: &str,
    bound: &std::collections::HashMap<String, Value>,
    answers_param: &str,
    label: &str,
) -> OperationWiring {
    let mut bound_params = bound.clone();
    bound_params.insert(
        answers_param.to_string(),
        Value::Array(vec![Value::String(label.to_string())]),
    );
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: EntityName::new(entity_name.to_string()),
            name: op_name.to_string(),
            trigger: Some(Trigger::Click {
                modifiers: ClickModifiers::none(),
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
                surface: holon_api::NonMenuSurface::PointerGesture,
            },
            guard: holon_api::pattern::OpGuard::None,
        },
    }
}

/// A `selectable` node showing one answer and carrying its wiring. Its own row
/// id keeps the platform layer's per-element identity distinct per answer.
fn answer_button(question_id: &str, label: &str, wiring: OperationWiring) -> ViewModel {
    let mut data = DataRow::new();
    data.insert(
        "id".to_string(),
        Value::String(format!("{question_id}#{label}")),
    );
    data.insert("label".to_string(), Value::String(label.to_string()));
    ViewModel {
        operations: vec![wiring],
        children: vec![Arc::new(ViewModel::text(label))],
        ..ViewModel::from_widget("selectable", std::collections::HashMap::new())
            .with_entity(Arc::new(data))
    }
}

/// The labels an explicitly bound `answers:` contributes to the offered-set
/// check. The override is guard-input only: dispatch always sends the clicked
/// button's own one-element array, never this value. Both the array and the
/// scalar shape must face the check — an unchecked one is the only route by
/// which a label the question never offered could reach dispatch.
fn forced_labels(bound: &Value) -> Vec<String> {
    match bound {
        Value::Array(items) => items.iter().map(Value::to_display_string).collect(),
        scalar => vec![scalar.to_display_string()],
    }
}

fn dotted(name: &str) -> (&str, &str) {
    name.split_once('.').unwrap_or_else(|| {
        panic!(
            "question_options: `action:` must name an entity operation in dot form \
             (`entity.op`), got `{name}` — an undotted name would target the wrong entity"
        )
    })
}

// Renders the answers a pending question offers, one clickable button each,
// wired to the `action:` operation with that answer's label.
//
// The offered set is the authority: a label the question does not offer is
// refused HERE and produces no wiring at all, so no click can reach the
// provider with it.
holon_macros::widget_builder! {
    fn question_options(
        options: String,
        action: Expr,
        #[default = "answers"] answers_param: String,
        #[default = "question_id"] id_param: String,
    ) -> ViewModel {
        let Some(RenderExpr::FunctionCall { name, args, .. }) = action else {
            return ViewModel::error(
                "question_options",
                "question_options requires an `action:` operation template".to_string(),
            );
        };
        let offered = match parse_offered(&options) {
            Ok(offered) => offered,
            Err(e) => return ViewModel::error("question_options", format!("question_options: {e}")),
        };

        let row = ba.ctx.row();
        let resolved = resolve_args(args, row);
        assert!(
            resolved.positional.is_empty(),
            "question_options: `action:` takes named params only, got {} positional",
            resolved.positional.len()
        );

        // An explicitly bound answer overrides what the widget would fill in —
        // the one route by which a label the question never offered could reach
        // dispatch. Refuse it visibly instead of wiring it up.
        if let Some(forced) = resolved.named.get(&answers_param) {
            for label in forced_labels(forced) {
                if !offered.iter().any(|o| o.label == label) {
                    let offers: Vec<&str> = offered.iter().map(|o| o.label.as_str()).collect();
                    return ViewModel::error(
                        "question_options",
                        format!(
                            "refusing to answer `{label}`: the question does not offer it \
                             (offered: {})",
                            offers.join(", ")
                        ),
                    );
                }
            }
        }

        let (entity_name, op_name) = dotted(name);
        // The mirror stores the primary key scheme-qualified
        // (`cc-pending-question:<question_id>`); the tool wants the provider's
        // opaque id, which is the URI's path. Read fresh from the row on every
        // build — the id is volatile and nothing may cache it.
        let row_id = row
            .get("id")
            .and_then(|v| v.as_string())
            .expect("question_options: the row must carry the question's `id`");
        let question_id = holon_api::EntityUri::parse(row_id)
            .unwrap_or_else(|e| panic!("question_options: row id must be an entity URI: {e}"))
            .id()
            .to_string();

        let items = offered
            .iter()
            .map(|answer| {
                let mut bound = resolved.named.clone();
                bound.insert(id_param.clone(), Value::String(question_id.clone()));
                let wiring =
                    answer_wiring(entity_name, op_name, &bound, &answers_param, &answer.label);
                answer_button(&question_id, &answer.label, wiring)
            })
            .collect();
        ViewModel::static_collection("question_options", items, 8.0)
    }
}
