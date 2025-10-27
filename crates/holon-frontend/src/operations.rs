use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_eval::eval_to_value;
use holon_api::render_types::{OperationDescriptor, OperationWiring, RenderExpr};
use holon_api::widget_spec::DataRow;
use holon_api::{EntityName, Value};

use crate::{FrontendSession, RenderContext};

pub fn dispatch_operation(
    handle: &tokio::runtime::Handle,
    session: &Arc<FrontendSession>,
    entity_name: &EntityName,
    op_name: String,
    params: HashMap<String, Value>,
) {
    let session = Arc::clone(session);
    let entity_name = entity_name.clone();
    handle.spawn(async move {
        if let Err(e) = session
            .execute_operation(&entity_name, &op_name, params)
            .await
        {
            tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
        }
    });
}

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
    /// Build an intent for an operation that takes an `id` param from the current row.
    pub fn for_row(
        op: &OperationDescriptor,
        row_id: &str,
        entity_name_override: Option<&EntityName>,
    ) -> Self {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(row_id.to_string()));
        Self {
            entity_name: entity_name_override
                .unwrap_or_else(|| &op.entity_name)
                .clone(),
            op_name: op.name.clone(),
            params,
        }
    }

    /// Build a `set_field` intent (used by state_toggle, editable_text on blur, etc.).
    pub fn set_field(
        entity_name: &EntityName,
        op_name: &str,
        row_id: &str,
        field: &str,
        value: Value,
    ) -> Self {
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

/// Filter operations whose `affected_fields` intersect with the given field list.
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

/// Find the value-setting operation for `field` on this widget.
///
/// State_toggle, editable_text, etc. need to dispatch a write of a specific
/// value into `field`. The canonical op for that is the generic `set_field`
/// (which takes id/field/value params); we prefer that. As a fallback we
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

/// Extract the entity name from the current row's ID scheme (e.g. `"block:uuid"` → `"block"`),
/// falling back to an explicit `entity_name` field.
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
