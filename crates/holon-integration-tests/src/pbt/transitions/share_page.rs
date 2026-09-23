//! Transition: the owner shares ONE page with the receiver through the
//! production per-page share, and the receiver accepts it.
//!
//! @pbt kind transition
//! @pbt covers two-instance-page-share — `share_subtree` on the owner plus
//!   `accept_shared_subtree` on the receiver: the placement record (the mount)
//!   the Overlay proposal's increment 1 turns into the page's own identity.
//!
//! **Exclusive with `ShareContainer`.** Whole-store pairing refuses a receiver
//! that holds a mount (D73.a, ADR 0033 §5), so a draw that did both would judge
//! a state production refuses to enter.
//!
//! The receiver accepts under the first of the model's receiver pages; a later
//! `MovePlacedRoot` is what moves it.

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
#[step_template("I share page {page} with the receiver")]
pub struct SharePage {
    pub page: EntityUri,
}

/// Where the receiver accepts every per-page share.
fn accept_parent<R: RefSharedView>(state: &R) -> EntityUri {
    state
        .receiver_pages()
        .into_iter()
        .next()
        .expect("the model names at least one receiver page")
}

impl<R: RefSharedView + RefSharedViewMut> TransitionFactory<R> for SharePage {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pages = state.shareable_pages();
        check(
            !state.is_shared() && state.page_shares().is_empty() && !pages.is_empty(),
            Reason::PreconditionFailed,
        )
        .map(|()| {
            // Heavy for the same reason as `ShareContainer`: a once-per-run
            // event that must come early to leave ticks for the placement moves.
            (
                30,
                proptest::sample::select(pages)
                    .prop_map(|page| SharePage { page })
                    .boxed(),
            )
        })
    }
}

impl<R: RefSharedView + RefSharedViewMut> TransitionRef<R> for SharePage {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.is_shared(), Reason::PreconditionFailed),
            check(state.page_shares().is_empty(), Reason::PreconditionFailed),
            check(
                state.shareable_pages().contains(&self.page),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        let parent = accept_parent(state);
        state.note_page_share(self.page.clone(), parent);
    }
}

crate::cap_transition! {
    SharePage: SutTwoInstance,
    where R: [ RefSharedView + RefSharedViewMut ],
    |me, state, sut| {
        sut.share_page(&me.page, &accept_parent(state)).await;
    }
    sql_budget: |_me, _state| {
        // The share runs a fork-and-prune on the owner, re-projects the page into
        // the owner's SQL, and the accept writes the RECEIVER's engine, which the
        // owner-side span collector does not trace.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 64,
        }
    }
}
