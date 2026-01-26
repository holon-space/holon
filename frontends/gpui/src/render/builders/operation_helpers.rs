pub use holon_frontend::operations::find_set_field_op;

use holon_api::render_types::{OperationDescriptor, OperationWiring};
use holon_api::Value;
use holon_frontend::ViewModel;

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

pub fn entity_name_from_node(node: &ViewModel) -> Option<String> {
    if let Some(Value::String(s)) = node.entity.get("entity_name") {
        return Some(s.clone());
    }
    node.operations
        .first()
        .map(|ow| ow.descriptor.entity_name.to_string())
}

pub fn row_id_from_node(node: &ViewModel) -> Option<String> {
    match node.entity.get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}
