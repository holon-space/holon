//! The dual-consumer certificate for `MarkingDelta` (ADR 0032 §4), the sibling
//! of `descriptor_arcs_roundtrip.rs`.
//!
//! A declared delta is data an external tool reads — the derived projection,
//! the simulator, whatever draws the net. A declaration that does not survive
//! serialize → deserialize is loadable by exactly one consumer, which is the
//! shape ADR 0031's dual-consumer requirement rules out.

use std::collections::HashMap;

use holon_api::ArcRelation;
use holon_api::BoundaryBehavior;
use holon_api::ExistenceFlow;
use holon_api::KindDelta;
use holon_api::MarkingDelta;
use holon_api::MenuExposure;
use holon_api::NonMenuSurface;
use holon_api::OperationDescriptor;
use holon_api::StructuralFlow;
use holon_api::TargetScope;
use holon_api::TextFlow;
use holon_api::TransitionArcs;
use holon_api::pattern::OpGuard;

fn descriptor_with(marking_delta: MarkingDelta) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: "move_block".to_string(),
        display_name: "Move block".to_string(),
        description: "Move a block to a new placement".to_string(),
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
        marking_delta,
        guard: OpGuard::None,
        arcs: TransitionArcs::Undeclared,
    }
}

fn declared() -> MarkingDelta {
    MarkingDelta::Envelope {
        kinds: vec![KindDelta {
            kind: ArcRelation::block(),
            structural: StructuralFlow::Relocates,
            text: TextFlow::Produces,
            existence: ExistenceFlow::Reads,
        }],
        varies_by: vec!["field".to_string()],
    }
}

#[test]
fn a_declared_delta_survives_a_serde_round_trip() {
    let original = descriptor_with(declared());

    let json = serde_json::to_string(&original).expect("serialize descriptor");
    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");

    assert_eq!(back.marking_delta, declared());
    assert_eq!(back, original, "so does the rest of the descriptor");
}

/// `Undeclared` is a stated fact and must serialize as one — not as an absent
/// field a lenient consumer could read as "changes nothing".
#[test]
fn undeclared_is_serialized_as_a_stated_fact() {
    let json = serde_json::to_string(&descriptor_with(MarkingDelta::Undeclared))
        .expect("serialize descriptor");
    assert!(
        json.contains(r#""marking_delta":{"kind":"undeclared"}"#),
        "the refusal to declare is on the wire: {json}"
    );

    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");
    assert_eq!(back.marking_delta.kinds(), None);
}

/// `Static` and `Envelope` carry different obligations, so they must not
/// collapse into one wire shape: an envelope read back as static would claim
/// every firing moves what only some firings move.
#[test]
fn static_and_envelope_have_different_wire_shapes() {
    let kinds = vec![KindDelta {
        kind: ArcRelation::block(),
        structural: StructuralFlow::Produces,
        text: TextFlow::Produces,
        existence: ExistenceFlow::Produces,
    }];
    let static_json = serde_json::to_string(&MarkingDelta::Static {
        kinds: kinds.clone(),
    })
    .expect("serialize");
    let envelope_json = serde_json::to_string(&MarkingDelta::Envelope {
        kinds,
        varies_by: vec!["fields".to_string()],
    })
    .expect("serialize");

    assert_ne!(static_json, envelope_json);
}
