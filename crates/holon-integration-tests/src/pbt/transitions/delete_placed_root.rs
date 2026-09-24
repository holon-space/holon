//! Transition: the receiver deletes a page the owner shared with it.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — a recipient delete of a placed page,
//!   which leaves the share on the receiver and never deletes the owner's page.

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
#[step_template("I delete shared page {page} on the receiver (whole subtree: {subtree})")]
pub struct DeletePlacedRoot {
    pub page: EntityUri,
    /// `delete_subtree` rather than the bare `delete`: the two reach the
    /// authority by different routes.
    pub subtree: bool,
}

/// Shared pages the receiver still holds.
fn held_pages<R: RefSharedView>(state: &R) -> Vec<EntityUri> {
    state
        .page_shares()
        .into_iter()
        .filter(|(_, share)| !share.left)
        .map(|(page, _)| page)
        .collect()
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for DeletePlacedRoot {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pages = held_pages(state);
        check(!pages.is_empty(), Reason::PreconditionFailed).map(|()| {
            // Light: leaving ends every later placement move, so a heavy weight
            // would starve `MovePlacedRoot`.
            (
                5,
                (proptest::sample::select(pages), any::<bool>())
                    .prop_map(|(page, subtree)| DeletePlacedRoot { page, subtree })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for DeletePlacedRoot {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(
            held_pages(state).contains(&self.page),
            Reason::PreconditionFailed,
        )
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.note_placed_root_left(&self.page);
    }
}

crate::cap_transition! {
    DeletePlacedRoot: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.delete_on_receiver(&me.page, me.subtree).await;
    }
    sql_budget: |_me, _state| {
        // The delete lands in the RECEIVER's engine, which the owner-side span
        // collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
