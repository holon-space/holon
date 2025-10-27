use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_eval::eval_to_value;
use holon_api::render_types::{OperationDescriptor, OperationWiring, RenderExpr};
use holon_api::widget_spec::DataRow;
use holon_api::Value;

use crate::{FrontendSession, RenderContext};

pub fn dispatch_operation(
    handle: &tokio::runtime::Handle,
    session: &Arc<FrontendSession>,
    entity_name: String,
    op_name: String,
    params: HashMap<String, Value>,
) {
    let session = Arc::clone(session);
    handle.spawn(async move {
        if let Err(e) = session
            .execute_operation(&entity_name, &op_name, params)
            .await
        {
            tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
        }
    });
}

pub fn dispatch_undo(handle: &tokio::runtime::Handle, session: &Arc<FrontendSession>) {
    let session = Arc::clone(session);
    handle.spawn(async move {
        if let Err(e) = session.undo().await {
            tracing::error!("Undo failed: {e}");
        }
    });
}

pub fn dispatch_redo(handle: &tokio::runtime::Handle, session: &Arc<FrontendSession>) {
    let session = Arc::clone(session);
    handle.spawn(async move {
        if let Err(e) = session.redo().await {
            tracing::error!("Redo failed: {e}");
        }
    });
}

/// Parsed action from a `selectable(child, action:(entity.operation ...))` expression.
pub struct ParsedAction {
    pub entity_name: String,
    pub op_name: String,
    pub params: HashMap<String, Value>,
}

/// Parse a RenderExpr action into entity name, operation name, and parameters.
///
/// Expects a `FunctionCall` whose name is `"entity.operation"` (dot-separated).
/// Named arguments are evaluated against the current data row.
pub fn parse_action_expr(action_expr: &RenderExpr, row: &DataRow) -> Option<ParsedAction> {
    if let RenderExpr::FunctionCall {
        name,
        args: action_args,
        ..
    } = action_expr
    {
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 2 {
            let entity_name = parts[0].to_string();
            let op_name = parts[1].to_string();

            let mut params = HashMap::new();
            for arg in action_args {
                if let Some(ref param_name) = arg.name {
                    let value = eval_to_value(&arg.value, row);
                    params.insert(param_name.clone(), value);
                }
            }

            return Some(ParsedAction {
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

pub fn find_set_field_op<'a>(
    field: &str,
    ops: &'a [OperationWiring],
) -> Option<&'a OperationDescriptor> {
    if let Some(ow) = ops
        .iter()
        .find(|ow| ow.descriptor.affected_fields.contains(&field.to_string()))
    {
        return Some(&ow.descriptor);
    }
    if let Some(ow) = ops.iter().find(|ow| ow.descriptor.name == "set_field") {
        return Some(&ow.descriptor);
    }
    None
}

pub fn get_entity_name(ctx: &RenderContext) -> Option<String> {
    if let Some(Value::String(s)) = ctx.row().get("entity_name") {
        return Some(s.clone());
    }
    ctx.operations
        .first()
        .map(|ow| ow.descriptor.entity_name.to_string())
}

pub fn get_row_id(ctx: &RenderContext) -> Option<String> {
    match ctx.row().get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}
