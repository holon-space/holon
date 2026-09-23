//! Transition: the receiver moves a page the owner shared with it.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — a recipient `move_block` of a shared
//!   page root, which moves the receiver's own placement record and never the
//!   owner's page.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::RefSharedViewMut;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I move shared page {page} under {new_parent} on the receiver")]
pub struct MovePlacedRoot {
    pub page: EntityUri,
    pub new_parent: EntityUri,
}

/// Every `(page, receiver page)` pair that would actually move the page.
fn candidate_moves<R: RefSharedView>(state: &R) -> Vec<(EntityUri, EntityUri)> {
    let pages = state.receiver_pages();
    state
        .page_shares()
        .into_iter()
        .flat_map(|(page, share)| {
            pages
                .iter()
                .filter(move |p| **p != share.receiver_parent)
                .map(move |p| (page.clone(), p.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for MovePlacedRoot {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let moves = candidate_moves(state);
        check(!moves.is_empty(), Reason::PreconditionFailed).map(|()| {
            (
                20,
                proptest::sample::select(moves)
                    .prop_map(|(page, new_parent)| MovePlacedRoot { page, new_parent })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for MovePlacedRoot {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(
            candidate_moves(state).contains(&(self.page.clone(), self.new_parent.clone())),
            Reason::PreconditionFailed,
        )
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.note_placed_root_move(&self.page, self.new_parent.clone());
    }
}

crate::cap_transition! {
    MovePlacedRoot: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.move_on_receiver(&me.page, &me.new_parent).await;
    }
    sql_budget: |_me, _state| {
        // The move lands in the RECEIVER's engine, which the owner-side span
        // collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
