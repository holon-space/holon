//! ADR 0031's dual-consumer certificate: an `OperationDescriptor`'s guard must
//! survive a serde round-trip.
//!
//! "The catalog substrate must be loadable by BOTH the in-memory engine and the
//! real dispatcher" — so the guard has to be ordinary serializable data. While
//! the field was a `dyn Fn` this test could not pass: the round-tripped
//! descriptor carried no guard at all, and the serialized JSON had no guard
//! key.

use std::collections::HashMap;

use holon_api::BoundaryBehavior;
use holon_api::MenuExposure;
use holon_api::NonMenuSurface;
use holon_api::OperationDescriptor;
use holon_api::TargetScope;
use holon_api::pattern::OpGuard;

fn descriptor_with(guard: OpGuard) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: "add_tag".to_string(),
        display_name: "Add tag".to_string(),
        description: "Add one tag to a block".to_string(),
        required_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        menu_exposure: MenuExposure::NotListed {
            surface: NonMenuSurface::Internal,
        },
        boundary_behavior: BoundaryBehavior::PrivateOnly,
        target_scope: TargetScope::Block,
        trigger: None,
        bound_params: HashMap::new(),
        guard,
    }
}

/// The declared guard must still be there after serialize → deserialize.
#[test]
fn descriptor_guard_survives_a_serde_round_trip() {
    let declared = OpGuard::parse("has_tag(\"Page\") and parent(not has_tag(\"Page\"))")
        .expect("the guard parses");
    let with_guard = descriptor_with(declared.clone());

    let json = serde_json::to_string(&with_guard).expect("serialize descriptor");
    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");

    assert_eq!(
        back.guard, declared,
        "the declared guard must survive the round-trip; it is the dual-consumer \
         certificate (ADR 0031). json was: {json}"
    );
    assert_eq!(back, with_guard, "the whole descriptor round-trips");
}

/// The gate's refusal quotes the developer's own `#[require]` text, so the
/// source literal is part of the declaration and must round-trip with it.
#[test]
fn declared_guard_carries_its_source_text_across_a_round_trip() {
    let src = "has_tag(\"Page\") and parent(not has_tag(\"Page\"))";
    let declared = OpGuard::parse(src).expect("the guard parses");
    assert_eq!(declared.source(), Some(src), "the parser keeps the literal");

    let json = serde_json::to_string(&descriptor_with(declared)).expect("serialize");
    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.guard.source(),
        Some(src),
        "the source literal must survive the round-trip; the refusal message \
         quotes it. json was: {json}"
    );
}

/// "Declares no precondition" is a stated fact that also round-trips — absent
/// is not the same as unstated.
#[test]
fn descriptor_without_a_guard_states_that_explicitly() {
    let none = descriptor_with(OpGuard::None);

    let json = serde_json::to_string(&none).expect("serialize descriptor");
    assert!(
        json.contains(r#""guard":{"kind":"none"}"#),
        "the absence is serialized as a stated fact: {json}"
    );

    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");
    assert_eq!(back.guard, OpGuard::None);
}
