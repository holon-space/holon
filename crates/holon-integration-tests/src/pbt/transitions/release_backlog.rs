//! Transition: the held read-only files become readable, and the backlog
//! drains.
//!
//! @pbt rung external
//!   releases the in-memory vault's `.cook` reads and waits for
//!   `VaultBacklogDrained` (`SutAppLifecycle::release_backlog`).
//! @pbt covers boot-writable-phase-first — the backlog the boot left behind
//!   is ingested by the watch loop once its files can be read
//!
//! Hand-authored only, after [`super::RebootWithBacklogHeld`].

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the read-only files can be read again")]
pub struct ReleaseBacklog;

impl<R: RefLifecycle> TransitionFactory<R> for ReleaseBacklog {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl<R: RefLifecycle> TransitionRef<R> for ReleaseBacklog {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    /// The recipe ingests to what the store already holds.
    fn apply_to_ref(&self, _: &mut R) {}
}

crate::cap_transition! {
    ReleaseBacklog: SutAppLifecycle,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.release_backlog().await;
    }
    sql_budget: |_me, state| {
        // The re-ingest of an unchanged recipe, inside the window.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 40 + crate::pbt::transition_budgets::docs_tolerance(state),
        }
    }
}
