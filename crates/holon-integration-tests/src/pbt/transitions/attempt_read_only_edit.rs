//! Transition: aim a store-origin content write at a block homed in a
//! `WriteTier::ReadOnly` file.
//!
//! @pbt rung storage
//! @pbt covers read-only-home-write-boundary — the write the dispatcher must
//!   refuse, without which `inv-read-only-home-refuses-writes` can only prove
//!   that nothing tried
//!
//! This is the one transition that WANTS to be refused. It dispatches through
//! the production operation dispatcher exactly as a user's edit does; the
//! outcome is recorded, not asserted, so the judgment stays in the invariant
//! where the disclosure half lives too.
//!
//! The oracle applies it as a NO-OP: a refused write leaves the store holding
//! what the file said, which is what every block invariant then keeps
//! comparing against.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutReadOnlyEditAttempt;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Write `content` onto the read-only-homed block `block_id`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I try to write {content} onto the read-only-homed block {block_id}")]
pub struct AttemptReadOnlyEdit {
    pub block_id: String,
    pub content: String,
}

impl TransitionFactory<ReferenceState> for AttemptReadOnlyEdit {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // The refusal happens in the dispatcher, above any projection, but the
        // block it names is only in the store on a Turso draw — the same arm
        // whose fixture seeds the read-only document.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let targets: Vec<String> = state
            .read_only
            .homes()
            .iter()
            .map(|id| id.to_string())
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            // No read-only home in this draw's fixture: narrow out rather than
            // aim a "must be refused" write at a writable block.
            check(!targets.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(targets)
                .prop_flat_map(|block_id| {
                    proptest::string::string_regex("[a-z]{1,8}")
                        .expect("valid regex")
                        .prop_map(move |content| AttemptReadOnlyEdit {
                            block_id: block_id.clone(),
                            content,
                        })
                })
                .boxed();
            // Low weight: one refused write per run is enough to arm the
            // invariant, and every draw spent here is a draw not spent on the
            // editing alphabet the keystone exists for.
            (4, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for AttemptReadOnlyEdit {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state
                    .read_only
                    .homes()
                    .iter()
                    .any(|id| id.as_str() == self.block_id),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.read_only.record_attempt();
    }
}

crate::cap_transition! {
    AttemptReadOnlyEdit: SutReadOnlyEditAttempt,
    where R: [ RefReadOnlyHomes ],
    |me, _state, sut| {
        // The refusal is the expected outcome and is judged by
        // `inv-read-only-home-refuses-writes`; a write that succeeds here must
        // reach that invariant, not die in an assert.
        let _ = sut.attempt_read_only_edit(&me.block_id, &me.content).await;
    }
    sql_budget: |_me, _state| {
        // The gate answers above every provider: one block read to resolve the
        // block's owning document, no write at all.
        ExpectedSql {
            reads: 1,
            writes: 0,
            ddl: 0,
            tolerance: 32,
        }
    }
}
