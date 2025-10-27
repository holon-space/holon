use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::{OperationDescriptor, OperationWiring};
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

pub fn get_entity_name<Ext: Clone>(ctx: &RenderContext<Ext>) -> Option<String> {
    if let Some(Value::String(s)) = ctx.row().get("entity_name") {
        return Some(s.clone());
    }
    ctx.operations
        .first()
        .map(|ow| ow.descriptor.entity_name.to_string())
}

pub fn get_row_id<Ext: Clone>(ctx: &RenderContext<Ext>) -> Option<String> {
    match ctx.row().get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}
