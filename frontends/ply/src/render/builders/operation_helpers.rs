#![allow(dead_code)]

use holon_api::render_types::{OperationDescriptor, OperationWiring};
use holon_api::Value;

use super::super::context::RenderContext;

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
