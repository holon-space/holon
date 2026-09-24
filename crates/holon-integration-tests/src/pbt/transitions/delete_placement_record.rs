//! Transition: the receiver deletes the placement record of a page the owner
//! shared with it — the mount that places the page — by the record's own id.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — a recipient delete naming a page's
//!   mount rather than the page, which leaves the page's share exactly as a
//!   delete of the page does and never deletes the owner's page.

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
#[step_template("I delete the receiver's placement record of shared page {page} by its own id")]
pub struct DeletePlacementRecord {
    pub page: EntityUri,
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

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for DeletePlacementRecord {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pages = held_pages(state);
        check(!pages.is_empty(), Reason::PreconditionFailed).map(|()| {
            // Light, as `DeletePlacedRoot` is: leaving ends every later
            // placement move.
            (
                3,
                proptest::sample::select(pages)
                    .prop_map(|page| DeletePlacementRecord { page })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for DeletePlacementRecord {
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
    DeletePlacementRecord: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.delete_placement_record_on_receiver(&me.page).await;
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
