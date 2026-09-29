//! Transition: a second session on the live vault is refused.
//!
//! @pbt rung dispatch
//!   `assert_second_writer_refused` boots a second session in-process through
//!   the production entry point.
//! @pbt covers vault-single-writer — a second session on a held vault is
//! refused by the writer lock, names the holder and writes nothing
//!
//! Model.md invariant 4 at the process boundary: two sessions on one vault
//! each save their whole Loro document, so the last saver drops the other's
//! edits. The live session holds `{vault}/.holon/writer.lock`; the second boot
//! must fail before it opens anything.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Boot a second session over the live vault and assert the lock refuses it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("a second writer on the vault is refused")]
pub struct SecondWriterRefused;

impl<R: RefLifecycle> TransitionFactory<R> for SecondWriterRefused {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // The refusal reads the live session's on-disk vault paths, which only
        // the Turso-backed headless component exposes.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        SecondWriterRefused
            .preconditions(state)
            .map(|()| (1, Just(SecondWriterRefused).boxed()))
    }
}

impl<R: RefLifecycle> TransitionRef<R> for SecondWriterRefused {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut R) {
        // A refused boot changes nothing.
    }
}

crate::cap_transition! {
    SecondWriterRefused: SutAppLifecycle,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.assert_second_writer_refused().await;
    }
    sql_budget: |_me, _state| {
        // The refused boot fails before it opens a DB. Measured 3 reads, all
        // from the live session's own background work in the window.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4,
        }
    }
}
