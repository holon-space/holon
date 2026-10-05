//! Transition: two single-op edits of one decision, dispatched concurrently,
//! the first parked after its shape judgement until the second has queued.
//!
//! @pbt rung dispatch
//! @pbt covers shape-claim-ordering — a judged write holds the decision
//!   subtree it judged until it has written, so a second write on that
//!   subtree is judged against the first one's result, never beside it
//!
//! The oracle applies the first edit and then the second, each as
//! `EditDecisionSubtree` would: the correct SUT serializes them in that order.
//! Each edit plans exactly one op, so it runs as a single gesture, not a
//! plan, and only the shape claim orders the two.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::ExpectedShapeOutcome;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::edit_decision_subtree::DecisionEdit;
use super::edit_decision_subtree::EditDecisionSubtree;
use super::edit_decision_subtree::decision_roots;
use super::edit_decision_subtree::edit_strategy;
use super::edit_decision_subtree::removal_writable;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I hold {first} on decision {root} after its judgement while {second} runs")]
pub struct EditDecisionPairHeld {
    pub root: String,
    pub first: DecisionEdit,
    pub second: DecisionEdit,
}

impl EditDecisionPairHeld {
    fn edits(&self) -> [EditDecisionSubtree; 2] {
        [&self.first, &self.second].map(|edit| EditDecisionSubtree {
            root: self.root.clone(),
            edit: edit.clone(),
        })
    }

    /// Whether `edit` plans one op in `state`, and that op's outcome; applies
    /// it to `state`.
    fn single_op(edit: &EditDecisionSubtree, state: &mut ReferenceState) -> Option<bool> {
        if matches!(edit.edit, DecisionEdit::SourceTextAsAgent { .. })
            || !edit.preconditions(state).is_good()
        {
            return None;
        }
        edit.apply_to_ref(state);
        let applied = state.shape.outcomes().last() == Some(&ExpectedShapeOutcome::Applied);
        (state.shape.planned().len() == 1).then_some(applied)
    }

    /// The first edit lands in the model; it must, to be parked after its
    /// judgement.
    fn plans(&self, state: &ReferenceState) -> bool {
        let [first, second] = self.edits();
        let mut state = state.clone();
        Self::single_op(&first, &mut state) == Some(true)
            && Self::single_op(&second, &mut state).is_some()
    }
}

impl TransitionFactory<ReferenceState> for EditDecisionPairHeld {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        EditDecisionSubtree::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let roots = decision_roots(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!roots.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let writable = removal_writable(state);
            let strat = (
                proptest::sample::select(roots),
                edit_strategy(writable),
                edit_strategy(writable),
            )
                .prop_map(|(root, first, second)| EditDecisionPairHeld {
                    root,
                    first,
                    second,
                })
                .boxed();
            (10, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for EditDecisionPairHeld {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(self.plans(state), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        for edit in self.edits() {
            edit.apply_to_ref(state);
        }
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutShapeEdit> holon_pbt_core::TransitionImpl<ReferenceState, S> for EditDecisionPairHeld {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let [first, second] = self.edits();
        let [first_ops, second_ops] = state.shape.last_planned(2) else {
            unreachable!("last_planned(2) holds two plans")
        };
        let only = |ops: &[holon_api::Operation]| {
            let [op] = ops else {
                panic!("a held pair's edit plans one op, got {ops:?}")
            };
            op.clone()
        };
        sut.apply_held_shape_pair(
            (only(first_ops), first.origin()),
            (only(second_ops), second.origin()),
        )
        .await;
    }
}

#[cfg(feature = "otel-testing")]
impl holon_pbt_core::budget::SqlBudget for EditDecisionPairHeld {
    fn expected_sql<R2: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R2,
    ) -> holon_pbt_core::budget::ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        sql.tolerance += 80;
        sql
    }
}
