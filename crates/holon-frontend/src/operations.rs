use std::collections::HashMap;
use std::sync::Arc;

use holon_api::BlockWriteFieldError;
use holon_api::EntityName;
use holon_api::InterpValue;
use holon_api::Value;
use holon_api::block_write_field::refuse_structural_field;
use holon_api::render_eval::EvalEnv;
use holon_api::render_eval::eval_to_interp;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::spawner::Spawner;

use crate::FrontendSession;
use crate::RenderContext;

/// Every click-bound intent on a node, keyed by the modifier set that selects
/// it — the lookup table a click handler consults on mouse-down.
///
/// Taking the operations SLICE rather than a node keeps this usable from both
/// view-model representations and from the tree-walking helpers in
/// `focus_path`, which is what stops each frontend hand-rolling the same
/// `filter_map` over `descriptor.click_modifiers()`. Adding a modifier is then
/// a profile-YAML entry plus a shadow-builder wiring, never a change in a
/// platform builder.
pub fn click_intents(
    ops: &[OperationWiring],
) -> HashMap<holon_api::ClickModifiers, OperationIntent> {
    ops.iter()
        .filter_map(|ow| {
            ow.descriptor.click_modifiers().map(|m| {
                (
                    m,
                    OperationIntent::new(
                        ow.descriptor.entity_name.clone(),
                        ow.descriptor.name.clone(),
                        ow.descriptor.bound_params.clone(),
                    ),
                )
            })
        })
        .collect()
}

/// The single click intent bound to `modifiers`, if any.
pub fn click_intent_for(
    ops: &[OperationWiring],
    modifiers: holon_api::ClickModifiers,
) -> Option<OperationIntent> {
    let op = ops
        .iter()
        .find(|ow| ow.descriptor.click_modifiers() == Some(modifiers))?;
    Some(OperationIntent::new(
        op.descriptor.entity_name.clone(),
        op.descriptor.name.clone(),
        op.descriptor.bound_params.clone(),
    ))
}

/// The `set_field` intent a `state_toggle` click must dispatch: look up the
/// setter op for `field`, advance `current` one step through `states`, and
/// address the write at `row_id`.
///
/// `states` is the comma-separated list the widget carries. `entity_name` is
/// the node's own entity when it has one, otherwise the op's declared entity.
/// `Ok(None)` means the toggle is not wired for writing — the caller discloses
/// that, it is not an error here. `Err` means `field` is not writable by
/// `set_field` at all.
pub fn state_toggle_intent(
    field: &str,
    current: &str,
    states: &str,
    ops: &[OperationWiring],
    entity_name: Option<&EntityName>,
    row_id: Option<&str>,
) -> Result<Option<OperationIntent>, BlockWriteFieldError> {
    let (Some(op), Some(row_id)) = (find_set_field_op(field, ops), row_id) else {
        return Ok(None);
    };
    let states_vec: Vec<String> = states.split(',').map(|s| s.trim().to_string()).collect();
    let next = holon_api::render_eval::cycle_state(current, &states_vec);
    let entity_name = entity_name.unwrap_or(&op.entity_name);
    OperationIntent::set_field(entity_name, &op.name, row_id, field, Value::String(next)).map(Some)
}

/// The bool-bound counterpart of [`state_toggle_intent`]: flip `current` and
/// dispatch the decision at its own type.
///
/// Results as in [`state_toggle_intent`].
pub fn state_toggle_intent_bool(
    field: &str,
    current: bool,
    ops: &[OperationWiring],
    entity_name: Option<&EntityName>,
    row_id: Option<&str>,
) -> Result<Option<OperationIntent>, BlockWriteFieldError> {
    let (Some(op), Some(row_id)) = (find_set_field_op(field, ops), row_id) else {
        return Ok(None);
    };
    let entity_name = entity_name.unwrap_or(&op.entity_name);
    OperationIntent::set_field(
        entity_name,
        &op.name,
        row_id,
        field,
        Value::Boolean(!current),
    )
    .map(Some)
}

pub fn dispatch_operation(
    spawner: &Arc<dyn Spawner>,
    session: &Arc<FrontendSession>,
    entity_name: &EntityName,
    op_name: String,
    params: HashMap<String, Value>,
) {
    let session = Arc::clone(session);
    let entity_name = entity_name.clone();
    // End-to-end latency: start the interaction clock at the dispatch entry
    // point; `holon_api::latency_e2e` closes it when the target's row lands
    // in a LiveData mirror (stage="e2e").
    let latency_target = params
        // ALLOW(raw_row_id_column): param-map — an op intent's params, read to start the latency
        // clock
        .get("id")
        .and_then(|v| v.as_string())
        .map(String::from);
    if let Some(target) = &latency_target {
        holon_api::latency_e2e::interaction_dispatched(
            &op_name,
            target,
            holon_api::latency_e2e::Observable::BlockRow(
                holon_api::latency_e2e::write_seq_from_params(&params),
            ),
            holon_api::latency_e2e::ClockOrigin::Ui,
        );
    }
    let run = session.execute_operation(&entity_name, &op_name, params);
    spawner.spawn(Box::pin(async move {
        if let Err(e) = run.await {
            // A refused/failed op writes nothing: retire its latency entry so no
            // later unrelated delivery for the row closes it as a phantom sample.
            if let Some(target) = &latency_target {
                holon_api::latency_e2e::interaction_failed(
                    &op_name,
                    target,
                    holon_api::latency_e2e::ClockOrigin::Ui,
                );
            }
            session.error_tracker().record_error();
            tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
        }
    }));
}

// TODO: How does this relate to MatchedOperation? Please DRY and SRP if
// possible
/// A fully-resolved intent to execute an operation.
///
/// Produced by UI interaction handlers (click, blur, menu select) and consumed
/// by `BuilderServices::dispatch_intent()`. Separating intent construction from
/// dispatch makes the "user clicked X → operation Y" path testable without a
/// running UI framework.
#[derive(Debug, Clone)]
pub struct OperationIntent {
    pub entity_name: EntityName,
    pub op_name: String,
    pub params: HashMap<String, Value>,
    /// Set only by the editor, for a commit of its source channel. Private,
    /// so no other caller can claim the keystroke exemption of Model.md
    /// invariant 17.
    keystroke: bool,
}

impl OperationIntent {
    pub fn new(entity_name: EntityName, op_name: String, params: HashMap<String, Value>) -> Self {
        Self {
            entity_name,
            op_name,
            params,
            keystroke: false,
        }
    }

    /// An intent the editor built from its own text. A `set_field` of the
    /// source line is the editor's keystroke; anything else is an ordinary
    /// intent.
    pub(crate) fn from_editor(
        entity_name: EntityName,
        op_name: String,
        params: HashMap<String, Value>,
    ) -> Self {
        let keystroke = op_name == "set_field"
            && params.get("field").and_then(|v| v.as_string())
                == Some(holon_api::SOURCE_TEXT_FIELD);
        Self {
            entity_name,
            op_name,
            params,
            keystroke,
        }
    }

    /// The editor keystroke this intent carries, if it is one.
    pub fn keystroke(&self) -> Option<holon_api::SourceKeystroke> {
        if !self.keystroke {
            return None;
        }
        let text = |key: &str| {
            self.params
                .get(key)
                .and_then(|v| v.as_string())
                .map(str::to_string)
                .unwrap_or_else(|| panic!("an editor keystroke intent carries its `{key}`"))
        };
        Some(holon_api::SourceKeystroke {
            id: text("id"),
            source: text("value"),
            write_seq: self.params.get("write_seq").and_then(|v| v.as_i64()),
        })
    }

    /// Convert from an `Operation` (the value returned by macro-generated
    /// `*_op()` constructors) by dropping the `display_name` field.
    /// `display_name` is only used for UI labels of pending/registered ops;
    /// once an op is built and ready to dispatch, only `(entity_name,
    /// op_name, params)` matter to the executor.
    pub fn from_operation(op: holon_api::Operation) -> Self {
        Self::new(op.entity_name, op.op_name, op.params)
    }
}

impl From<holon_api::Operation> for OperationIntent {
    fn from(op: holon_api::Operation) -> Self {
        Self::from_operation(op)
    }
}

impl OperationIntent {
    /// Build an intent for an operation that takes an `id` param from the
    /// current row.
    pub fn for_row(
        op: &OperationDescriptor,
        row_id: &str,
        entity_name_override: Option<&EntityName>,
    ) -> Self {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(row_id.to_string()));
        Self::new(
            entity_name_override.unwrap_or(&op.entity_name).clone(),
            op.name.clone(),
            params,
        )
    }

    /// Build a `set_field` intent (used by state_toggle, editable_text on blur,
    /// etc.). `field` can come from author data (a profile's `lane_field` or
    /// `state_toggle` field), so a private field or an order key is refused
    /// here (Model.md invariants 3 and 16): it needs a structural op instead.
    pub fn set_field(
        entity_name: &EntityName,
        op_name: &str,
        row_id: &str,
        field: &str,
        value: Value,
    ) -> Result<Self, BlockWriteFieldError> {
        refuse_structural_field(field)?;
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(row_id.to_string()));
        params.insert("field".to_string(), Value::String(field.to_string()));
        params.insert("value".to_string(), value);
        Ok(Self::new(entity_name.clone(), op_name.to_string(), params))
    }

    /// The web editor's flush of a block's text. It writes the `content`
    /// column, never the source line, so the web frontend has no keystroke
    /// channel (Model.md invariant 17).
    pub fn content_edit(block_id: &str, content: &str) -> Self {
        Self::set_field(
            &EntityName::new("block"),
            "set_field",
            block_id,
            "content",
            Value::String(content.to_string()),
        )
        .expect("content is a writable field")
    }

    /// The `{entity, op, params}` object the web frontend posts to its worker.
    pub fn to_wire(&self) -> serde_json::Value {
        serde_json::json!({
            "entity": self.entity_name.to_string(),
            "op": self.op_name,
            "params": self.params,
        })
    }

    /// The worker's read of [`Self::to_wire`]. A wire intent is never a
    /// keystroke.
    pub fn from_wire(item: &serde_json::Value) -> anyhow::Result<Self> {
        let field = |key: &str| {
            item.get(key)
                .ok_or_else(|| anyhow::anyhow!("intent missing '{key}': {item}"))
        };
        let entity = field("entity")?
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("intent 'entity' is not text: {item}"))?;
        let op = field("op")?
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("intent 'op' is not text: {item}"))?;
        let params: HashMap<String, Value> = serde_json::from_value(field("params")?.clone())
            .map_err(|e| anyhow::anyhow!("parse intent params of {item}: {e}"))?;
        Ok(Self::new(EntityName::from(entity), op.to_string(), params))
    }
}

/// Parse a RenderExpr action into entity name, operation name, and parameters.
///
/// Expects a `FunctionCall` whose name is `"entity.operation"` (dot-separated).
/// Named arguments are evaluated against `ctx`'s row, one by one: a param
/// named like a render template (`parent_id`, `action`) is still a value.
pub fn parse_action_expr(
    action_expr: &RenderExpr,
    services: &dyn crate::reactive::BuilderServices,
    ctx: &RenderContext,
) -> Result<Option<OperationIntent>, holon_api::computation::ComputeError> {
    if let RenderExpr::FunctionCall {
        name,
        args: action_args,
        ..
    } = action_expr
    {
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 2 {
            let entity_name = EntityName::Named(parts[0].to_string());
            let op_name = parts[1].to_string();

            let fns = services.value_fn_lookup(ctx);
            let env = EvalEnv::of_row(ctx.row());
            let mut params = HashMap::new();
            for arg in action_args {
                if let Some(ref param_name) = arg.name {
                    let value = match eval_to_interp(&arg.value, &env, &*fns)
                        .map_err(|e| e.placed_in(|| format!("arg `{param_name}`")))?
                    {
                        InterpValue::Value(value) => value,
                        InterpValue::Rows(_) => {
                            return Err(holon_api::computation::ComputeError::WrongType {
                                context: format!("operation param `{param_name}`"),
                                expected: "a value, not a row set",
                                value: Value::Null,
                            });
                        }
                    };
                    params.insert(param_name.clone(), value);
                }
            }

            return Ok(Some(OperationIntent::new(entity_name, op_name, params)));
        }
    }
    Ok(None)
}

/// Filter operations whose `affected_fields` intersect with the given field
/// list.
pub fn find_ops_affecting<'a>(
    fields: &[&str],
    ops: &'a [OperationWiring],
) -> Vec<&'a OperationDescriptor> {
    ops.iter()
        .filter(|ow| {
            ow.descriptor
                .affected_fields
                .iter()
                .any(|af| fields.contains(&af.as_str()))
        })
        .map(|ow| &ow.descriptor)
        .collect()
}

// NOTE: `set_field` is NOT obsolete. The Loro/`MutableText` "field-in-sync-
// with-UI" mechanism is the *implementation underneath* `set_field`, not a
// replacement for it: `SqlBlockOperations::set_field` routes writes through
// the `BlockCellRegistry` (content → LoroText, parent_id → tree.mov, the rest
// → LoroMap meta), and the `LoroSyncController` outbound projector emits the
// SQL UPDATE. The registry returns `false` for SqlOnly mode, synthetic test
// stores, and fields with no clean Loro encoding (`sort_key`, `depth`), where
// `set_field` falls back to a direct SQL write. So `set_field` remains the
// canonical field-write seam across both backends. This function finds the
// matching `set_field` *operation descriptor* so the frontend can dispatch a
// value write from `state_toggle`/`editable_text` widgets.
/// Find the value-setting operation for `field` on this widget.
///
/// State_toggle, editable_text, etc. need to dispatch a write of a specific
/// value into `field`. The canonical op for that is the generic `set_field`
/// (which takes id/field/value params); we prefer that. Otherwise we
/// accept any op whose `affected_fields` covers `field` AND that takes a
/// `value` parameter — i.e. an actual setter, not a side-effecting trigger.
///
/// Without the `value`-param check, ops like `cycle_task_state` (which
/// declares `affected_fields = ["task_state"]` but takes only `id`) would
/// be matched here, and a dispatch from `state_toggle` would end up cycling
/// rather than setting the chosen state.
pub fn find_set_field_op<'a>(
    field: &str,
    ops: &'a [OperationWiring],
) -> Option<&'a OperationDescriptor> {
    if let Some(ow) = ops.iter().find(|ow| ow.descriptor.name == "set_field") {
        return Some(&ow.descriptor);
    }
    ops.iter()
        .find(|ow| {
            ow.descriptor.affected_fields.contains(&field.to_string())
                && ow
                    .descriptor
                    .required_params
                    .iter()
                    .any(|p| p.name == "value")
        })
        .map(|ow| &ow.descriptor)
}

/// Extract the entity name from the current row's ID scheme (e.g.
/// `"block:uuid"` → `"block"`), falling back to an explicit `entity_name`
/// field.
pub fn get_entity_name(ctx: &RenderContext) -> Option<String> {
    // ALLOW(raw_row_id_column): key — reads the SCHEME only, to name the entity the
    // row belongs to
    if let Some(Value::String(id)) = ctx.row().get("id") {
        if let Some((scheme, _)) = id.split_once(':') {
            return Some(scheme.to_string());
        }
    }
    if let Some(Value::String(s)) = ctx.row().get("entity_name") {
        return Some(s.clone());
    }
    None
}

/// The entity the current row names. `None` both when the row carries no
/// `id` and when the id forms no URI — a builder that dispatches on the row
/// has nothing to address in either case.
pub fn get_row_id(ctx: &RenderContext) -> Option<holon_api::EntityUri> {
    holon_api::row_id_of(ctx.row()).entity()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_field_intent_over_a_structural_field_is_refused() {
        for (field, expected) in [
            (
                "sort_key",
                BlockWriteFieldError::Private {
                    field: "sort_key".to_string(),
                    route: "move_block { id, parent_id, after_block_id }",
                },
            ),
            (
                "parent_id",
                BlockWriteFieldError::Private {
                    field: "parent_id".to_string(),
                    route: "move_block { id, parent_id, after_block_id }",
                },
            ),
            (
                "after_block_id",
                BlockWriteFieldError::OrderKey("after_block_id".to_string()),
            ),
        ] {
            let refused = OperationIntent::set_field(
                &EntityName::Named("block".to_string()),
                "set_field",
                "block:a",
                field,
                Value::String("block:b".to_string()),
            );
            assert_eq!(refused.unwrap_err(), expected, "{field}");
        }
    }

    /// A bool-bound toggle carries a typed decision on the wire. The word
    /// vocabulary is the other binding's; sending `"on"` here would make the
    /// provider parse a string back into the bool it already had.
    #[test]
    fn a_bool_bound_toggle_dispatches_a_boolean() {
        let ops = [OperationDescriptor {
            entity_name: EntityName::Named("integration".to_string()),
            entity_short_name: "integration".to_string(),
            id_column: "id".to_string(),
            name: "set_field".to_string(),
            display_name: String::new(),
            description: String::new(),
            required_params: vec![],
            optional_params: vec![],
            affected_fields: vec!["enabled".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: HashMap::new(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
        .to_default_wiring()];

        let intent = state_toggle_intent_bool(
            "enabled",
            false,
            &ops,
            Some(&EntityName::Named("integration".to_string())),
            Some("integration:gmail"),
        )
        .expect("`enabled` is writable")
        .expect("a wired bool toggle must produce an intent");

        assert_eq!(
            intent.params.get("value"),
            Some(&Value::Boolean(true)),
            "toggling a stored `false` must dispatch Value::Boolean(true), not a state word"
        );
    }

    #[test]
    fn set_field_intent_over_content_constructs() {
        let intent = OperationIntent::set_field(
            &EntityName::Named("block".to_string()),
            "set_field",
            "block:a",
            "content",
            Value::String("hello".to_string()),
        )
        .expect("content is writable");
        assert_eq!(intent.op_name, "set_field");
    }

    /// Only the editor can mark a source-line write as its keystroke; the
    /// same params built by anyone else stay an ordinary, judged intent.
    #[test]
    fn only_the_editor_marks_a_source_write_as_a_keystroke() {
        let params = HashMap::from([
            ("id".to_string(), Value::String("block:d".into())),
            (
                "field".to_string(),
                Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
            ),
            ("value".to_string(), Value::String("TODO x".into())),
            ("write_seq".to_string(), Value::Integer(7)),
        ]);
        let by_name = OperationIntent::new("block".into(), "set_field".into(), params.clone());
        assert_eq!(by_name.keystroke(), None);
        let from_editor = OperationIntent::from_editor("block".into(), "set_field".into(), params);
        assert_eq!(
            from_editor.keystroke(),
            Some(holon_api::SourceKeystroke {
                id: "block:d".into(),
                source: "TODO x".into(),
                write_seq: Some(7),
            })
        );
    }
}
