//! `inv-conditions-match-ref` — a refusal the user was never told about is a
//! silent failure, not a safe one.
//!
//! @pbt oracle model-equivalence — the conditions in effect on the production
//!   bus equal the ones the reference model expects, within the kinds the model
//!   claims authority over
//! @pbt covers disclosure-of-a-refused-write — a transition that fails on
//!   purpose must leave a condition raised, not merely decline to write
//! @pbt slips-if-removed a gate keeps refusing writes while its `disclose` call
//!   is gone, so the store correctly declines and the user sees nothing — which
//!   from inside the store is indistinguishable from a write that worked
//! @pbt slips-if-removed an all-clear stops firing, so a resolved degradation
//!   stays on screen forever and the banner becomes noise the user learns to
//!   ignore
//!
//! **Scope.** The model governs KINDS, not the whole bus. A booted app
//! legitimately discloses things no transition caused — secrets held in memory,
//! undo history cleared at boot — so comparing the entire set would fail on
//! facts the model has no opinion about. Within a governed kind the comparison
//! is exact in BOTH directions: a missing disclosure and a spurious one are
//! both failures.
//!
//! Subjects are matched by FILE NAME, anchored to a path component. Most raise
//! sites carry an absolute path inside the run's temp vault, which no constant
//! can know; the model pins the file name and the invariant honours that rather
//! than pinning nothing. The comparison is identity at the FINAL PATH
//! COMPONENT, never a raw string suffix — an unanchored `ends_with` lets a
//! different file called `a-keystone-recipe.cook` satisfy an expectation for
//! `keystone-recipe.cook`, so the pinned file's refusal goes undisclosed and
//! this invariant reports a clean run. See [`subject_matches_expected`] and
//! bugfunnel entry
//! `2026-09-15-conditions-invariant-matches-subject-by-raw-suffix`.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RaisedCondition;
use holon_pbt_core::capabilities::RefConditions;
use holon_pbt_core::capabilities::SutConditions;
use holon_pbt_core::capabilities::subject_matches_expected;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvConditionsMatchRef;

impl InvConditionsMatchRef {
    pub const ID: InvariantId = InvariantId("inv-conditions-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvConditionsMatchRef
where
    R: RefConditions,
    S: SutConditions,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let governed: BTreeSet<&'static str> = reference.governed_condition_kinds();
        if governed.is_empty() {
            // No failing transition has run, so the model claims nothing. This
            // is a deselection, not a pass: asserting "the app is quiet" here
            // would fail on every legitimate boot-time disclosure.
            return InvariantResult::Ok;
        }

        let expected = reference.expected_conditions();
        let raised: Vec<RaisedCondition> = sut
            .conditions_now()
            .await
            .into_iter()
            .filter(|c| governed.contains(c.kind.as_str()))
            .collect();

        let missing: Vec<String> = expected
            .iter()
            .filter(|(name, kind)| {
                !raised
                    .iter()
                    .any(|r| r.kind == *kind && subject_matches_expected(&r.subject, name))
            })
            .map(|(name, kind)| {
                format!("{kind} on a subject whose final path component is `{name}`")
            })
            .collect();
        if !missing.is_empty() {
            return InvariantResult::Fail(format!(
                "{} expected disclosure(s) are NOT in effect: {:?}. The transition's outcome \
                 happened and the user was never told, which from inside the store is \
                 indistinguishable from it having succeeded. Raised (governed kinds only): {:?}",
                missing.len(),
                missing,
                raised,
            ));
        }

        let spurious: Vec<&RaisedCondition> = raised
            .iter()
            .filter(|r| {
                !expected.iter().any(|(name, kind)| {
                    r.kind == *kind && subject_matches_expected(&r.subject, name)
                })
            })
            .collect();
        if !spurious.is_empty() {
            return InvariantResult::Fail(format!(
                "{} condition(s) of a governed kind are in effect that the model does not expect: \
                 {:?}. Either a degradation happened that no transition caused, or an all-clear \
                 that should have fired did not — a stale banner the user learns to ignore. \
                 Expected: {:?}",
                spurious.len(),
                spurious,
                expected,
            ));
        }

        InvariantResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use holon_pbt_core::capabilities::RaisedCondition;
    use holon_pbt_core::capabilities::RefConditions;
    use holon_pbt_core::capabilities::SutConditions;

    use super::*;
    use crate::pbt::conditions_state::ConditionsRefState;

    /// The stable kind `read_only_format_gate` discloses on a refused write.
    const KIND: &str = holon_api::ConditionKind::EDIT_REFUSED_READ_ONLY_FORMAT;
    /// What `AttemptReadOnlyEdit` pins in the model: a BARE FILE NAME.
    const PINNED_NAME: &str = "keystone-recipe.cook";
    /// The temp-vault path the gate raises for the pinned file.
    const PINNED_PATH: &str = "/tmp/vault/keystone-recipe.cook";
    /// A DIFFERENT file in the same vault whose name merely ENDS WITH the
    /// pinned one. No constant can tell it apart from the pinned file by
    /// string suffix alone; a path component can.
    const COLLIDING_PATH: &str = "/tmp/vault/a-keystone-recipe.cook";

    struct RefStub {
        expected: Vec<(String, &'static str)>,
        governed: BTreeSet<&'static str>,
    }

    impl RefStub {
        /// The `AttemptReadOnlyEdit` shape: one pinned name, one governed kind.
        fn pinning_the_recipe() -> Self {
            Self {
                expected: vec![(PINNED_NAME.to_string(), KIND)],
                governed: [KIND].into_iter().collect(),
            }
        }
    }

    impl RefConditions for RefStub {
        fn expected_conditions(&self) -> Vec<(String, &'static str)> {
            self.expected.clone()
        }

        fn governed_condition_kinds(&self) -> BTreeSet<&'static str> {
            self.governed.clone()
        }
    }

    struct SutStub {
        raised: Vec<RaisedCondition>,
    }

    impl SutStub {
        fn raising(subjects: &[&str]) -> Self {
            Self {
                raised: subjects
                    .iter()
                    .map(|subject| RaisedCondition {
                        subject: (*subject).to_string(),
                        kind: KIND.to_string(),
                    })
                    .collect(),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl SutConditions for SutStub {
        async fn conditions_now(&self) -> Vec<RaisedCondition> {
            self.raised.clone()
        }
    }

    async fn check(subjects: &[&str]) -> InvariantResult {
        InvConditionsMatchRef
            .check(&RefStub::pinning_the_recipe(), &SutStub::raising(subjects))
            .await
    }

    /// REGRESSION —
    /// `2026-09-15-conditions-invariant-matches-subject-by-raw-suffix`.
    ///
    /// A subject whose FINAL PATH COMPONENT differs from the pinned name must
    /// NOT satisfy the expectation, even though its string ends with it.
    /// `a-keystone-recipe.cook` is a DIFFERENT file: the pinned file's refusal
    /// went untold, the user was never told, and an unanchored `ends_with` said
    /// `Ok` — exactly the silent failure this invariant exists to catch.
    #[tokio::test]
    async fn a_different_file_whose_name_ends_with_the_pinned_one_does_not_satisfy_it() {
        let InvariantResult::Fail(message) = check(&[COLLIDING_PATH]).await else {
            panic!(
                "`{COLLIDING_PATH}` merely ENDS WITH the pinned `{PINNED_NAME}`, so it is a \
                 DIFFERENT file: the invariant must report the pinned file's disclosure as \
                 missing instead of accepting the collision"
            );
        };
        assert!(
            message.contains(PINNED_NAME),
            "the failure must name the file whose refusal went undisclosed; got: {message}"
        );
        assert!(
            message.contains("NOT in effect"),
            "the collision must surface on the MISSING side — the pinned file's refusal was \
             never told — not as a spurious condition; got: {message}"
        );
    }

    /// Positive control: the gate's actual raise for the pinned file satisfies
    /// the expectation. Without this, the test above could pass by matching
    /// nothing at all.
    #[tokio::test]
    async fn the_pinned_file_itself_satisfies_the_expectation() {
        assert!(
            matches!(check(&[PINNED_PATH]).await, InvariantResult::Ok),
            "the temp-vault path of the pinned file must satisfy the pinned name"
        );
    }

    /// The same collision in the spurious direction: a governed-kind condition
    /// on a DIFFERENT file is precisely a condition "the model does not
    /// expect". An unanchored suffix match swallows it and reports `Ok`.
    #[tokio::test]
    async fn a_colliding_sibling_beside_the_pinned_file_is_reported_as_spurious() {
        let InvariantResult::Fail(message) = check(&[PINNED_PATH, COLLIDING_PATH]).await else {
            panic!(
                "a governed-kind condition for `{COLLIDING_PATH}` is not expected by the model \
                 and must be reported as spurious"
            );
        };
        assert!(
            message.contains("does not expect"),
            "the colliding sibling must surface on the SPURIOUS side; got: {message}"
        );
    }

    /// The rule itself, stated once: identity at a path-component boundary.
    /// Exercised through the model's OWN `raise` path — the one
    /// `AttemptReadOnlyEdit::apply_to_ref` uses — so the expectation here is
    /// the production expectation, not a hand-built imitation.
    #[test]
    fn expectation_identity_is_the_final_path_component_not_a_string_suffix() {
        let mut model = ConditionsRefState::default();
        model.raise(PINNED_NAME, KIND);
        let matches = |subject: &str| model.expected().iter().any(|c| c.matches(subject));

        assert!(matches(PINNED_PATH), "the pinned file's own path matches");
        assert!(
            matches(PINNED_NAME),
            "a subject that is a bare name (no separator) matches whole"
        );
        assert!(
            !matches(COLLIDING_PATH),
            "a string-suffix collision is not identity"
        );
        assert!(
            !matches("/tmp/vault/keystone-recipe.cook.bak"),
            "a backup of the pinned file is a different file"
        );

        // The dot-boundary family too: `ends_with("cook")` accepts any `.cook`.
        let mut extension_only = ConditionsRefState::default();
        extension_only.raise("cook", KIND);
        let matches_extension =
            |subject: &str| extension_only.expected().iter().any(|c| c.matches(subject));
        assert!(matches_extension("/tmp/vault/cook"));
        assert!(
            !matches_extension("/tmp/vault/keystone-recipe.cook"),
            "`keystone-recipe.cook` is not the file named `cook`"
        );
    }
}
