//! Transition: the receiver files a page the owner shared with it under a new
//! page of its own, then deletes that page with everything under it.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — a recipient delete of a block the
//!   placed page hangs under, which leaves the page's share exactly as a
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
#[step_template(
    "I file shared page {page} under a new page below {receiver_parent} on the receiver, then \
     delete that page's subtree"
)]
pub struct DeletePlacementParent {
    pub page: EntityUri,
    /// Where the receiver holds the page now; the new page is created here.
    pub receiver_parent: EntityUri,
}

/// `(page, where the receiver holds it)` for every page the receiver holds.
fn held_placements<R: RefSharedView>(state: &R) -> Vec<(EntityUri, EntityUri)> {
    state
        .page_shares()
        .into_iter()
        .filter(|(_, share)| !share.left)
        .map(|(page, share)| (page, share.receiver_parent))
        .collect()
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for DeletePlacementParent {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let placements = held_placements(state);
        check(!placements.is_empty(), Reason::PreconditionFailed).map(|()| {
            // Light, as `DeletePlacedRoot` is: leaving ends every later
            // placement move.
            (
                5,
                proptest::sample::select(placements)
                    .prop_map(|(page, receiver_parent)| DeletePlacementParent {
                        page,
                        receiver_parent,
                    })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for DeletePlacementParent {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(
            held_placements(state).contains(&(self.page.clone(), self.receiver_parent.clone())),
            Reason::PreconditionFailed,
        )
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.note_placed_root_left(&self.page);
    }
}

crate::cap_transition! {
    DeletePlacementParent: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, _state, sut| {
        sut.delete_placement_parent_on_receiver(&me.page, &me.receiver_parent).await;
    }
    sql_budget: |_me, _state| {
        // The create, move and delete land in the RECEIVER's engine, which the
        // owner-side span collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
