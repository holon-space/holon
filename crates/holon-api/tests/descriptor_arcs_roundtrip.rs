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
            place(ArcRelation::Block, "content"),
            place(ArcRelation::Clock, "today"),
        ],
        emits: vec![
            ArcEmit::Writes(place(ArcRelation::Block, "content")),
            ArcEmit::Excluded {
                place: place(ArcRelation::Block, "sort_key"),
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

/// The drift lock between the arc vocabulary (`holon-pattern`, a leaf) and the
/// intent-write vocabulary (`BlockWriteField`, here). Both name what may be
/// written to a block; they cannot be one list because the leaf crate cannot
/// import this enum, so they are pinned to each other instead.
///
/// A new `BlockWriteField` variant without its arc place would make an op
/// unable to declare a write it can perform.
#[test]
fn intent_writable_fields_are_all_arc_places() {
    // Every raw name `BlockWriteField::parse` maps to a NAMED variant. An
    // unknown name parses as `Property`, which lands in `block.properties`, so
    // only the named ones need their own place.
    let named = [
        "content",
        "content_type",
        "source_language",
        "source_name",
        "marks",
        "collapsed",
        "widget_only",
        "completed",
        "block_type",
        "properties",
        "tags",
        "task_state",
        "parent_id",
    ];
    for raw in named {
        assert!(
            !matches!(
                holon_api::BlockWriteField::parse(raw),
                Ok(holon_api::BlockWriteField::Property(_)) | Err(_)
            ),
            "{raw:?} is listed here as a named BlockWriteField but no longer parses as one"
        );
        ArcPlace::parse(&format!("block.{raw}"))
            .unwrap_or_else(|e| panic!("intent-writable field {raw:?} has no arc place: {e}"));
    }

    // The other direction: an arc place for the block relation is either
    // intent-writable, an edge set, or deliberately read-only/excludable.
    let not_intent_writable = [
        "id",
        "sort_key",
        "after_block_id",
        "requires",
        "advice_suppressed",
    ];
    for field in ArcRelation::Block.known_fields() {
        if not_intent_writable.contains(field) {
            continue;
        }
        assert!(
            named.contains(field),
            "arc place block.{field} is neither an intent-writable BlockWriteField nor \
             listed as read-only/edge-set — the two vocabularies have drifted"
        );
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
