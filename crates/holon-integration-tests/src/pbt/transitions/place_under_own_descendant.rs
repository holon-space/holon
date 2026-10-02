//! Transition: dispatch `move_block` that names one of the block's own
//! descendants as the new parent.
//!
//! @pbt rung dispatch
//!   `attempt_place_under_own_descendant` dispatches at the production
//!   operation dispatcher, below every UI precondition.
//! @pbt covers parent-cycle-refused-at-every-placement-path — a cyclic move is
//!   refused on the SQL placement path as on the Loro one, so no store holds a
//!   parent cycle
//!
//! The oracle applies it as a NO-OP; `inv-no-parent-cycles` and the tree
//! comparisons judge the store.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutCyclicPlaceAttempt;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Move `id` under `descendant`, a block in `id`'s own subtree.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I dispatch a move of block {id} under its own descendant {descendant}")]
pub struct PlaceUnderOwnDescendant {
    pub id: EntityUri,
    pub descendant: EntityUri,
}

fn movable<R: RefBlockTree>(state: &R, id: &EntityUri) -> bool {
    state.is_text_block(id) && !state.is_page_block(id) && !state.is_layout_block(id)
}

fn is_strict_descendant<R: RefBlockTree>(
    state: &R,
    descendant: &EntityUri,
    ancestor: &EntityUri,
) -> bool {
    descendant != ancestor
        && state.is_descendant_of_any(descendant, &std::iter::once(ancestor.clone()).collect())
}

impl<R: RefLifecycle + RefBlockTree + RefLayout> TransitionFactory<R> for PlaceUnderOwnDescendant {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let all = state.all_block_ids();
        let pairs: Vec<PlaceUnderOwnDescendant> = all
            .iter()
            .filter(|id| movable(state, id))
            .flat_map(|id| {
                all.iter()
                    .filter(|d| is_strict_descendant(state, d, id))
                    .map(|d| PlaceUnderOwnDescendant {
                        id: id.clone(),
                        descendant: d.clone(),
                    })
            })
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!pairs.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            // Low weight: one refused move per run arms the rung, and every
            // draw spent here is a draw not spent on the editing alphabet.
            (2, proptest::sample::select(pairs).boxed())
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefLayout> TransitionRef<R> for PlaceUnderOwnDescendant {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(movable(state, &self.id), Reason::PreconditionFailed),
            check(
                is_strict_descendant(state, &self.descendant, &self.id),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // A refused move changes nothing.
    }
}

crate::cap_transition! {
    PlaceUnderOwnDescendant: SutCyclicPlaceAttempt,
    where R: [ RefLifecycle + RefBlockTree + RefLayout ],
    |me, _state, sut| {
        sut.attempt_place_under_own_descendant(&me.id, &me.descendant).await;
    }
    sql_budget: |_me, _state| {
        // The move's up-front reads (subject, predecessor, destination
        // existence and page-ness, ancestry), then the refusal: no write.
        ExpectedSql {
            reads: 6,
            writes: 0,
            ddl: 0,
            tolerance: 16,
        }
    }
}
