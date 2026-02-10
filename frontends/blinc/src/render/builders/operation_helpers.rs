use holon_api::render_types::{OperationDescriptor, OperationWiring};
use holon_api::Value;

use crate::render::context::RenderContext;

/// Find the operation for writing a specific field.
///
/// Priority: field-specific op → `set_field` for matching entity → any `set_field`.
pub fn find_set_field_op<'a>(
    field: &str,
    ops: &'a [OperationWiring],
) -> Option<&'a OperationDescriptor> {
    // 1. Field-specific operation
    if let Some(ow) = ops
        .iter()
        .find(|ow| ow.descriptor.affected_fields.contains(&field.to_string()))
    {
        return Some(&ow.descriptor);
    }
    // 2. Any set_field operation
    if let Some(ow) = ops.iter().find(|ow| ow.descriptor.name == "set_field") {
        return Some(&ow.descriptor);
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

/// Resolve entity name from the current row or operations.
pub fn get_entity_name(ctx: &RenderContext) -> Option<String> {
    if let Some(Value::String(s)) = ctx.row().get("entity_name") {
        return Some(s.clone());
    }
    ctx.operations
        .first()
        .map(|ow| ow.descriptor.entity_name.to_string())
}

/// Get the row's `id` field as a string.
pub fn get_row_id(ctx: &RenderContext) -> Option<String> {
    match ctx.row().get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}
