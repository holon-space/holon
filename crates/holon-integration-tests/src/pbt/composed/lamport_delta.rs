//! The exact Lamport advance a transition kind leaves on the SUT primary doc.
//!
//! The oracle pads its shadow primary to the SUT's fed height, so a SUT that
//! writes one Loro op too many or too few would be absorbed by the padding.
//! For the kinds whose advance is a constant, the harness asserts it exactly,
//! before that height is fed. A kind without a predictor stays unchecked; the
//! run lists those kinds once, derived from this table.
//!
//! @pbt kind oracle
//! @pbt covers lamport-delta — exact SUT Lamport delta per constant-delta kind

use std::sync::Once;

use holon_api::EdgeFieldUpdate;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::E2ETransition;
use crate::pbt::transitions::SetEdgeField;
use crate::pbt::transitions::SetTypedEntityField;

enum Delta {
    Constant(u32),
    /// One map set, or nothing when the write leaves the field as it was.
    EdgeFieldWrite,
    /// One map set, or nothing for a same-value write: Loro skips a map
    /// insert of the value the key already holds.
    TypedFieldWrite,
    /// The create's fixed ops plus one map set per written column. Every
    /// drawn id is fresh, so no tombstoned key is cleared first.
    TypedCreate,
}

/// Atom counts measured on the keystone (`just atom-counts`). `Outdent` has
/// none: a refused outdent of a page child writes nothing, and the post-state
/// cannot tell it from a successful one.
const PREDICTORS: &[(&str, Delta)] = &[
    ("Nothing", Delta::Constant(0)),
    ("CreateTypedEntity", Delta::TypedCreate),
    ("SetTypedEntityField", Delta::TypedFieldWrite),
    ("DeleteTypedEntity", Delta::Constant(1)),
    ("Indent", Delta::Constant(2)),
    ("SetEdgeField", Delta::EdgeFieldWrite),
    ("Search", Delta::Constant(0)),
    ("JumpToSearchHit", Delta::Constant(0)),
    ("NavigateFocus", Delta::Constant(0)),
    ("NavigateBack", Delta::Constant(0)),
    ("NavigateForward", Delta::Constant(0)),
    ("NavigateHome", Delta::Constant(0)),
    ("SwitchView", Delta::Constant(0)),
    ("MoveCursor", Delta::Constant(0)),
    ("ArrowNavigate", Delta::Constant(0)),
    ("WheelScroll", Delta::Constant(0)),
    ("AddPeer", Delta::Constant(0)),
    ("PeerEdit", Delta::Constant(0)),
];

/// Measured with one-, two- and four-column types.
const TYPED_CREATE_FIXED_OPS: u32 = 3;

/// The SUT primary's exact Lamport advance for `t`, read off the ref state
/// after `t`; `None` for a kind without a predictor.
pub fn predict(t: &E2ETransition, post: &ReferenceState) -> Option<u32> {
    static UNCHECKED: Once = Once::new();
    UNCHECKED.call_once(|| {
        eprintln!(
            "[lamport-delta] unchecked kinds (no predictor): {:?}",
            unchecked_kinds(E2ETransition::VARIANT_NAMES, PREDICTORS)
        );
    });
    let (_, delta) = PREDICTORS.iter().find(|(k, _)| *k == t.variant_name())?;
    Some(match delta {
        Delta::Constant(n) => *n,
        Delta::EdgeFieldWrite => {
            let E2ETransition::SetEdgeField(t) = t else {
                panic!("the edge-field predictor is keyed to SetEdgeField, got {t:?}")
            };
            edge_field_write(t, post)
        }
        Delta::TypedCreate => {
            let E2ETransition::CreateTypedEntity(t) = t else {
                panic!("the typed-create predictor is keyed to CreateTypedEntity, got {t:?}")
            };
            TYPED_CREATE_FIXED_OPS + t.fields.0.len() as u32
        }
        Delta::TypedFieldWrite => {
            let E2ETransition::SetTypedEntityField(t) = t else {
                panic!("the typed-field predictor is keyed to SetTypedEntityField, got {t:?}")
            };
            typed_field_write(t, post)
        }
    })
}

fn edge_field_write(t: &SetEdgeField, post: &ReferenceState) -> u32 {
    if post.shape.step_refusal().is_some() {
        return 0;
    }
    let pre = &post
        .action
        .undo_stack
        .last()
        .expect("SetEdgeField pushes its pre-write snapshot onto the undo stack")
        .blocks
        .blocks[&t.block_id];
    let unchanged = match &t.update {
        EdgeFieldUpdate::Tags(tags) => pre.tags == *tags,
        EdgeFieldUpdate::Requires(r) => pre.requires == *r,
        EdgeFieldUpdate::AdviceSuppressed(r) => pre.advice_suppressed == *r,
        EdgeFieldUpdate::ContributesTo(r) => pre.contributes_to == *r,
    };
    u32::from(!unchanged)
}

fn typed_field_write(t: &SetTypedEntityField, post: &ReferenceState) -> u32 {
    let column = post
        .typed_entities
        .columns(&t.type_name)
        .iter()
        .position(|c| *c == t.field)
        .unwrap_or_else(|| panic!("'{}' declares no column {:?}", t.type_name, t.field));
    let pre = &post
        .action
        .undo_stack
        .last()
        .expect("SetTypedEntityField pushes its pre-write snapshot onto the undo stack")
        .typed[&t.type_name][&t.id][column];
    u32::from(*pre != t.value)
}

fn unchecked_kinds(
    kinds: &'static [&'static str],
    predictors: &[(&str, Delta)],
) -> Vec<&'static str> {
    kinds
        .iter()
        .copied()
        .filter(|k| !predictors.iter().any(|(p, _)| p == k))
        .collect()
}

/// Panics when the SUT primary advanced by other than the predicted delta.
pub fn assert_delta(kind: &str, predicted: Option<u32>, before: Option<u32>, after: Option<u32>) {
    let (Some(want), Some(before), Some(after)) = (predicted, before, after) else {
        return;
    };
    let got = after.checked_sub(before).unwrap_or_else(|| {
        panic!(
            "[lamport-delta] '{kind}': the SUT Lamport height went back from {before} to {after}"
        )
    });
    assert_eq!(
        got,
        want,
        "[lamport-delta] '{kind}' advanced the SUT primary's Lamport clock by {got} \
         ({before} -> {after}), the model predicts exactly {want}: the SUT wrote \
         {} Loro op(s) the model does not expect",
        if got > want { "extra" } else { "fewer" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_predictor_names_a_real_kind() {
        for (kind, _) in PREDICTORS {
            assert!(
                E2ETransition::VARIANT_NAMES.contains(kind),
                "predictor for unknown kind {kind}"
            );
        }
    }

    #[test]
    fn adding_a_predictor_removes_the_kind_from_the_unchecked_list() {
        let unchecked = unchecked_kinds(E2ETransition::VARIANT_NAMES, PREDICTORS);
        assert!(unchecked.contains(&"SplitBlock"));
        assert!(!unchecked.contains(&"Indent"));
        assert_eq!(
            unchecked.len() + PREDICTORS.len(),
            E2ETransition::VARIANT_NAMES.len()
        );

        let mut more: Vec<(&str, Delta)> = PREDICTORS
            .iter()
            .map(|(k, d)| {
                (
                    *k,
                    match d {
                        Delta::Constant(n) => Delta::Constant(*n),
                        Delta::EdgeFieldWrite => Delta::EdgeFieldWrite,
                        Delta::TypedFieldWrite => Delta::TypedFieldWrite,
                        Delta::TypedCreate => Delta::TypedCreate,
                    },
                )
            })
            .collect();
        more.push(("SplitBlock", Delta::Constant(8)));
        let fewer = unchecked_kinds(E2ETransition::VARIANT_NAMES, &more);
        assert!(!fewer.contains(&"SplitBlock"));
        assert_eq!(fewer.len() + 1, unchecked.len());
    }

    #[test]
    #[should_panic(expected = "the SUT wrote extra Loro op(s)")]
    fn one_op_too_many_is_red() {
        assert_delta("Indent", Some(2), Some(10), Some(13));
    }

    #[test]
    #[should_panic(expected = "the SUT wrote fewer Loro op(s)")]
    fn one_op_too_few_is_red() {
        assert_delta("Indent", Some(2), Some(10), Some(11));
    }

    #[test]
    fn no_predictor_or_no_loro_log_checks_nothing() {
        assert_delta("SplitBlock", None, Some(10), Some(30));
        assert_delta("Indent", Some(2), None, None);
    }
}
