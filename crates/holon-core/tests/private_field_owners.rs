//! Model.md invariant 16, checked against the declared arcs: a private field
//! is written by the op its declaration names, and never by `set_field`.
//! `update` carries no arcs; the dispatcher refuses it instead
//! (`block_generic_writes_reject_private_fields_at_intent_boundary`).

use holon_api::ArcEmit;
use holon_api::OperationDescriptor;

fn block_catalog() -> Vec<OperationDescriptor> {
    let mut ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    ops.extend(holon_core::__operations_block_operations::block_operations(
        "block", "block", "block", "id",
    ));
    ops
}

fn op<'a>(ops: &'a [OperationDescriptor], name: &str) -> &'a OperationDescriptor {
    ops.iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("the block catalog has no {name}"))
}

fn emit_of<'a>(descriptor: &'a OperationDescriptor, field: &str) -> Option<&'a ArcEmit> {
    let place = format!("block.{field}");
    descriptor
        .arcs
        .emits()
        .unwrap_or_else(|| panic!("{} declares no arcs", descriptor.name))
        .iter()
        .find(|e| e.place().to_string() == place)
}

#[test]
fn private_fields_are_written_by_their_owner_and_excluded_from_set_field() {
    let ops = block_catalog();
    let private = holon_api::schema::BLOCK.private_fields();
    assert!(!private.is_empty(), "the lock would be vacuous");

    for (field, owner) in private {
        let route_op = owner
            .route
            .split_whitespace()
            .next()
            .expect("a route starts with its op name");
        assert!(
            matches!(emit_of(op(&ops, route_op), field), Some(ArcEmit::Writes(_))),
            "block.{field} is private to {route_op}, which does not declare that it writes it"
        );
        assert!(
            matches!(
                emit_of(op(&ops, "set_field"), field),
                Some(ArcEmit::Excluded { .. })
            ),
            "set_field must declare private block.{field} excluded"
        );
    }
}
