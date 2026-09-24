//! Transition: the receiver presses Backspace at the start of a page the owner
//! shared with it, then undoes whatever that did.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — a join that takes a placed page off
//!   the receiver without leaving its share, and an undo that brings the page
//!   back as a local block carrying the owner's id.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I join shared page {page} into the block above it on the receiver, then undo")]
pub struct JoinPlacedRoot {
    pub page: EntityUri,
}

fn held_pages<R: RefSharedView>(state: &R) -> Vec<EntityUri> {
    state
        .page_shares()
        .into_iter()
        .filter(|(_, share)| !share.left)
        .map(|(page, _)| page)
        .collect()
}

impl<R: RefSharedView> TransitionFactory<R> for JoinPlacedRoot {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pages = held_pages(state);
        check(!pages.is_empty(), Reason::PreconditionFailed).map(|()| {
            (
                3,
                proptest::sample::select(pages)
                    .prop_map(|page| JoinPlacedRoot { page })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView> TransitionRef<R> for JoinPlacedRoot {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(
            held_pages(state).contains(&self.page),
            Reason::PreconditionFailed,
        )
    }

    /// The join is refused, so the receiver still holds the page where it was.
    fn apply_to_ref(&self, _state: &mut R) {}
}

crate::cap_transition! {
    JoinPlacedRoot: SutTwoInstance,
    where R: [ RefSharedView ],
    |me, _state, sut| {
        sut.join_on_receiver(&me.page).await;
    }
    sql_budget: |_me, _state| {
        // The join lands in the RECEIVER's engine, which the owner-side span
        // collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}
