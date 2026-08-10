//! The dual-consumer certificate for `TransitionArcs` (ADR 0031 Increment 2),
//! the sibling of `descriptor_guard_roundtrip.rs`.
//!
//! ADR 0031 requires an op's declaration to be "loadable by BOTH the in-memory
//! engine and the real dispatcher" — which is why arcs are plain serializable
//! data. This test is what makes that claim checkable: a declaration that does
//! not survive serialize → deserialize is loadable by exactly one consumer.

use std::collections::HashMap;

use holon_api::ArcEmit;
use holon_api::ArcPlace;
use holon_api::ArcRelation;
use holon_api::BoundaryBehavior;
use holon_api::MenuExposure;
use holon_api::NonMenuSurface;
use holon_api::OperationDescriptor;
use holon_api::TargetScope;
use holon_api::TransitionArcs;
use holon_api::pattern::OpGuard;

fn descriptor_with(arcs: TransitionArcs) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: "set_field".to_string(),
        display_name: "Set field".to_string(),
        description: "Set one field on a block".to_string(),
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
        guard: OpGuard::None,
        arcs,
    }
}

fn place(relation: ArcRelation, field: &str) -> ArcPlace {
    ArcPlace {
        relation,
        field: field.to_string(),
    }
}

fn declared() -> TransitionArcs {
    TransitionArcs::Declared {
        reads: vec![
            place(ArcRelation::block(), "content"),
            place(ArcRelation::clock(), "today"),
        ],
        emits: vec![
            ArcEmit::Writes(place(ArcRelation::block(), "content")),
            ArcEmit::Excluded {
                place: place(ArcRelation::block(), "sort_key"),
                reason: "the ordering authority mints order keys".to_string(),
            },
        ],
    }
}

/// Every arc, its relation, and the exclusion REASON must still be there after
/// the round trip. The reason is the load-bearing part: an exclusion that
/// arrives without its justification is indistinguishable from silence.
#[test]
fn descriptor_arcs_survive_a_serde_round_trip() {
    let original = descriptor_with(declared());

    let json = serde_json::to_string(&original).expect("serialize descriptor");
    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");

    assert_eq!(back.arcs, declared(), "arcs survive the round trip");
    assert_eq!(back, original, "so does the rest of the descriptor");
}

/// `Undeclared` is a stated fact and must serialize as one — not as an absent
/// field a lenient consumer could read as "writes nothing".
#[test]
fn undeclared_arcs_are_serialized_as_a_stated_fact() {
    let undeclared = descriptor_with(TransitionArcs::Undeclared);

    let json = serde_json::to_string(&undeclared).expect("serialize descriptor");
    assert!(
        json.contains(r#""arcs":{"kind":"undeclared"}"#),
        "the refusal to declare is on the wire: {json}"
    );

    let back: OperationDescriptor = serde_json::from_str(&json).expect("deserialize descriptor");
    assert_eq!(back.arcs, TransitionArcs::Undeclared);
    assert_eq!(back.arcs.emits(), None, "and it is not an empty write set");
}

/// The intent vocabulary (`BlockWriteField`) against the ONE declaration it is
/// derived from. Not a hand-list on either side: the named variants are probed
/// by round-tripping every `FieldIntent::Writable` field, and the reverse
/// direction walks the declaration.
///
/// A named variant whose field stopped being declared writable, or a writable
/// field with no named variant, would silently land user writes in
/// `block.properties` instead of the column they name.
#[test]
fn intent_writable_fields_are_all_arc_places() {
    let writable = holon_api::schema::BLOCK.intent_writable();
    assert!(!writable.is_empty(), "the lock would be vacuous");

    for raw in &writable {
        let parsed = holon_api::BlockWriteField::parse(raw).unwrap_or_else(|e| {
            panic!("declared-writable field {raw:?} is refused by intent: {e}")
        });
        assert!(
            !matches!(parsed, holon_api::BlockWriteField::Property(_)),
            "{raw:?} is declared writable but falls through to a user property"
        );
        assert_eq!(
            parsed.as_str(),
            *raw,
            "the variant must write the field it parsed"
        );
        ArcPlace::parse(&format!("block.{raw}"))
            .unwrap_or_else(|e| panic!("intent-writable field {raw:?} has no arc place: {e}"));
    }

    // The other direction: a block arc place is either intent-writable or
    // deliberately not — and "not" means the intent boundary refuses it or
    // treats it as an ordinary property, never that it names a variant.
    for field in holon_api::schema::BLOCK.arc_places() {
        if writable.contains(&field) {
            continue;
        }
        match holon_api::BlockWriteField::parse(field) {
            Err(_) | Ok(holon_api::BlockWriteField::Property(_)) => {}
            Ok(named) => panic!(
                "block.{field} is not declared intent-writable but parses to the named variant                  {named:?} — the declaration and the vocabulary have drifted"
            ),
        }
    }
}

/// A declared-but-empty write set and `Undeclared` must not collapse into the
/// same wire shape — the whole point of the fail-closed variant is that a
/// second consumer can tell "writes nothing" from "cannot say".
#[test]
fn empty_emits_and_undeclared_have_different_wire_shapes() {
    let empty = TransitionArcs::Declared {
        reads: vec![],
        emits: vec![],
    };
    let empty_json = serde_json::to_string(&empty).expect("serialize");
    let undeclared_json = serde_json::to_string(&TransitionArcs::Undeclared).expect("serialize");

    assert_ne!(empty_json, undeclared_json);
    let back: TransitionArcs = serde_json::from_str(&empty_json).expect("deserialize");
    assert_eq!(back.emits(), Some(&[][..]));
}
