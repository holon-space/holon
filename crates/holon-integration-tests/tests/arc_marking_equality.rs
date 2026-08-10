//! The ADR 0031 marking-equality oracle, in embryo (Increment 2c).
//!
//! A declaration nobody checks is a lie waiting to happen. This is the first
//! consumer that checks one, and **both sides are live**:
//!
//! - the declaration side reads `set_field`'s `emits` off the real
//!   macro-generated descriptor, not a copy of it;
//! - the model side **calls `SetEdgeField::apply_to_ref` on a real
//!   `ReferenceState`** and derives the written places by diffing the subject
//!   block's serialized form before and after.
//!
//! The diff is what keeps this honest. Hand-transcribing "a `Tags` update
//! writes `block.tags`" would leave the oracle green forever if the reference
//! model started writing a second place — precisely the rot a dual-consumer
//! certificate exists to catch. Here, any field the model touches shows up as
//! a changed JSON key and must be a declared place or the test reds.
//!
//! The law is CONTAINMENT, not equality: `emits` is a static
//! over-approximation of a dynamic choice (`set_field`'s written place is its
//! `field` parameter), and over-approximation is what makes a declaration
//! sound for a simulator — it must never be surprised by a write it was not
//! told about. Dropping a real place therefore reds; adding a bogus one is
//! caught earlier, at macro-expansion time, by the closed field vocabulary in
//! `holon-pattern`'s `BLOCK_FIELDS` (containment alone could never see it).

use std::sync::Arc;

use holon_api::EdgeFieldUpdate;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::TransitionArcs;
use holon_api::block::Block;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::set_edge_field::SetEdgeField;
use holon_pbt_core::TransitionRef;

const SUBJECT: &str = "block:arc-oracle-subject";
const TARGET: &str = "block:arc-oracle-target";

/// The declared out-arcs of `set_field`, read from the real macro-generated
/// descriptor.
fn set_field_written_places() -> Vec<String> {
    let ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    let descriptor = ops
        .iter()
        .find(|op| op.name == "set_field")
        .expect("CrudOperations advertises set_field");
    assert!(
        matches!(descriptor.arcs, TransitionArcs::Declared { .. }),
        "set_field is the first op admitted to the exhaustiveness set (OQ-4); it \
         must not be Undeclared"
    );
    descriptor
        .arcs
        .written_places()
        .iter()
        .map(|p| p.to_string())
        .collect()
}

fn reference_with_two_blocks() -> ReferenceState {
    let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
    let mut state = ReferenceState::new(holon_pbt_core::Wiring::full(), interp);
    state.action.app_started = true;
    for id in [SUBJECT, TARGET] {
        let uri = EntityUri::parse(id).expect("a well-formed block uri");
        state.domain.block_state.blocks.insert(
            uri.clone(),
            Block::new_text(uri, EntityUri::no_parent(), "subject"),
        );
    }
    state
}

/// `Block` is deliberately serde-free; `BlockWire` is its sanctioned wire
/// boundary and — importantly here — it carries the junction-derived edge
/// fields a naive derive on `Block` would have dropped. That makes it the
/// right lens for "which places changed".
fn subject_snapshot(state: &ReferenceState) -> serde_json::Value {
    let uri = EntityUri::parse(SUBJECT).expect("a well-formed block uri");
    let block = state
        .domain
        .block_state
        .blocks
        .get(&uri)
        .expect("the subject block is seeded");
    serde_json::to_value(holon_api::block::BlockWire::from(block)).expect("a BlockWire serializes")
}

/// Run the REAL `apply_to_ref` and report which places it actually wrote,
/// derived from the subject block's own serialized shape. Nothing about the
/// `EdgeFieldUpdate` variants is transcribed here.
fn places_written_by_apply_to_ref(update: EdgeFieldUpdate) -> Vec<String> {
    let mut state = reference_with_two_blocks();
    let before = subject_snapshot(&state);

    SetEdgeField {
        block_id: EntityUri::parse(SUBJECT).expect("a well-formed block uri"),
        update,
    }
    .apply_to_ref(&mut state);

    let after = subject_snapshot(&state);

    let before = before.as_object().expect("a Block is a JSON object");
    let after = after.as_object().expect("a Block is a JSON object");
    let mut changed: Vec<String> = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| format!("block.{key}"))
        .collect();
    changed.sort();
    changed
}

fn every_edge_field_update() -> Vec<EdgeFieldUpdate> {
    let target = EntityUri::parse(TARGET).expect("a well-formed block uri");
    vec![
        EdgeFieldUpdate::Tags(Tags::from_csv("task,lesson")),
        EdgeFieldUpdate::Requires(vec![target.clone()]),
        EdgeFieldUpdate::AdviceSuppressed(vec![target]),
    ]
}

/// Every write the reference model actually performs for a `SetEdgeField`
/// transition must be a place `set_field` declared it emits.
#[test]
fn set_field_emits_cover_every_place_the_reference_writes() {
    let declared = set_field_written_places();
    assert!(
        !declared.is_empty(),
        "a declaration with no write places would satisfy nothing below"
    );

    let mut total_observed = 0usize;
    for update in every_edge_field_update() {
        let observed = places_written_by_apply_to_ref(update.clone());
        assert!(
            !observed.is_empty(),
            "apply_to_ref({update:?}) wrote NOTHING — the oracle would be vacuous. \
             Either the transition stopped working or the snapshot diff is blind."
        );
        for place in &observed {
            assert!(
                declared.contains(place),
                "the reference model writes {place} for {update:?}, but set_field does \
                 not declare it. A consumer simulating set_field from its declaration \
                 would miss that write. Declared: {declared:?}"
            );
        }
        total_observed += observed.len();
    }
    assert!(
        total_observed >= 3,
        "all three EdgeFieldUpdate variants must have been observed writing at \
         least one place each; saw {total_observed}"
    );
}

/// The diff really does see a write — pinned to the exact place each variant
/// lands on. This is a WITNESS, not the law above, and the two catch different
/// rot — each proven by its own mutation:
///
/// - the model grows a write to an UNDECLARED place → the **law** reds;
/// - the model grows a write to an already-declared place → containment still
///   holds, so the law stays green and this **witness** reds.
///
/// Neither alone is sufficient, which is why both exist.
#[test]
fn the_snapshot_diff_observes_the_expected_place_per_variant() {
    let target = EntityUri::parse(TARGET).expect("a well-formed block uri");
    assert_eq!(
        places_written_by_apply_to_ref(EdgeFieldUpdate::Tags(Tags::from_csv("task"))),
        vec!["block.tags".to_string()]
    );
    assert_eq!(
        places_written_by_apply_to_ref(EdgeFieldUpdate::Requires(vec![target.clone()])),
        vec!["block.requires".to_string()]
    );
    assert_eq!(
        places_written_by_apply_to_ref(EdgeFieldUpdate::AdviceSuppressed(vec![target])),
        vec!["block.advice_suppressed".to_string()]
    );
}

/// The exclusion mechanism is real: `set_field` declares the order keys
/// EXCLUDED, and an excluded place is NOT a declared write. If exclusions
/// leaked into `written_places` the law above would accept a write the
/// ordering authority alone is allowed to make.
#[test]
fn excluded_order_keys_are_not_declared_writes() {
    let declared = set_field_written_places();
    for order_key in ["block.sort_key", "block.after_block_id"] {
        assert!(
            !declared.contains(&order_key.to_string()),
            "{order_key} is declared EXCLUDED (Model.md invariant 3) and must not \
             appear as a write: {declared:?}"
        );
    }

    let ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    let descriptor = ops
        .iter()
        .find(|op| op.name == "set_field")
        .expect("CrudOperations advertises set_field");
    let excluded: Vec<String> = descriptor
        .arcs
        .emits()
        .expect("set_field is declared")
        .iter()
        .filter_map(|e| match e {
            holon_api::ArcEmit::Excluded { place, reason } => {
                assert!(
                    !reason.is_empty(),
                    "an exclusion without a reason is silence"
                );
                Some(place.to_string())
            }
            holon_api::ArcEmit::Writes(_) => None,
        })
        .collect();
    assert_eq!(
        excluded,
        vec![
            "block.sort_key".to_string(),
            "block.after_block_id".to_string()
        ],
        "both order keys are declared EXCLUDED, each with its reason"
    );
}
