use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_eval::eval_to_value;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;

use crate::FrontendSession;
use crate::RenderContext;

pub fn dispatch_operation(
    handle: &tokio::runtime::Handle,
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
        .get("id")
        .and_then(|v| v.as_string())
        .map(String::from);
    if let Some(target) = &latency_target {
        holon_api::latency_e2e::interaction_dispatched(
            &op_name,
            target,
            holon_api::latency_e2e::write_seq_from_params(&params),
        );
    }
    handle.spawn(async move {
        if let Err(e) = session
            .execute_operation(&entity_name, &op_name, params)
            .await
        {
            // A refused/failed op writes nothing: retire its latency entry so no
            // later unrelated delivery for the row closes it as a phantom sample.
            if let Some(target) = &latency_target {
                holon_api::latency_e2e::interaction_failed(&op_name, target);
            }
            session.error_tracker().record_error();
            tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
        }
    });
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
}

impl OperationIntent {
    pub fn new(entity_name: EntityName, op_name: String, params: HashMap<String, Value>) -> Self {
        Self {
            entity_name,
            op_name,
            params,
        }
    }

    /// Convert from an `Operation` (the value returned by macro-generated
    /// `*_op()` constructors) by dropping the `display_name` field.
    /// `display_name` is only used for UI labels of pending/registered ops;
    /// once an op is built and ready to dispatch, only `(entity_name,
    /// op_name, params)` matter to the executor.
    pub fn from_operation(op: holon_api::Operation) -> Self {
        Self {
            entity_name: op.entity_name,
            op_name: op.op_name,
            params: op.params,
        }
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
        Self {
            entity_name: entity_name_override.unwrap_or(&op.entity_name).clone(),
            op_name: op.name.clone(),
            params,
        }
    }

    /// Build a `set_field` intent (used by state_toggle, editable_text on blur,
    /// etc.).
    pub fn set_field(
        entity_name: &EntityName,
        op_name: &str,
        row_id: &str,
        field: &str,
        value: Value,
    ) -> Self {
        // Model.md invariant 3: intent never carries an order key. A widget
        // constructing a set_field over `sort_key`/`after_block_id` is a
        // programming error — reorders are expressed positionally through
        // structural ops (`move_block` with an `after_block_id` anchor) so
        // the ordering authority mints the key. Assert here so the bug
        // surfaces at the constructor, not as a downstream dispatch Err.
        assert!(
            !matches!(field, "sort_key" | "after_block_id"),
            "OperationIntent::set_field({field:?}): intent must never carry an order key \
             (Model.md invariant 3); dispatch a structural move (move_block) instead"
        );
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(row_id.to_string()));
        params.insert("field".to_string(), Value::String(field.to_string()));
        params.insert("value".to_string(), value);
        Self {
            entity_name: entity_name.clone(),
            op_name: op_name.to_string(),
            params,
        }
    }
}

/// Parse a RenderExpr action into entity name, operation name, and parameters.
///
/// Expects a `FunctionCall` whose name is `"entity.operation"` (dot-separated).
/// Named arguments are evaluated against the current data row.
pub fn parse_action_expr(action_expr: &RenderExpr, row: &DataRow) -> Option<OperationIntent> {
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

            let mut params = HashMap::new();
            for arg in action_args {
                if let Some(ref param_name) = arg.name {
                    let value = eval_to_value(&arg.value, row);
                    params.insert(param_name.clone(), value);
                }
            }

            return Some(OperationIntent {
                entity_name,
                op_name,
                params,
            });
        }
    }
    None
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

pub fn get_row_id(ctx: &RenderContext) -> Option<String> {
    match ctx.row().get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Model.md invariant 3: no widget may construct a `set_field` intent
    /// carrying an order key — the constructor asserts immediately instead
    /// of letting the smuggle travel to a downstream dispatch Err.
    #[test]
    #[should_panic(expected = "intent must never carry an order key")]
    fn set_field_intent_over_sort_key_is_unconstructible() {
        let _ = OperationIntent::set_field(
            &EntityName::Named("block".to_string()),
            "set_field",
            "block:a",
            "sort_key",
            Value::String("A5".to_string()),
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
        );
        assert_eq!(intent.op_name, "set_field");
    }
}
