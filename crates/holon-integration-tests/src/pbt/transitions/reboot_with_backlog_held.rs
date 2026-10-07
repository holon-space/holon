//! Transition: reboot with every read of a read-only (`.cook`) file held, so
//! the boot's read-only backlog cannot move.
//!
//! @pbt rung external
//!   holds the in-memory vault's `.cook` reads across the shutdown and boots
//!   again over the same store (`SutAppLifecycle::reboot_with_backlog_held`).
//! @pbt covers boot-writable-phase-first — the org files are ingested and the
//!   watch loop runs while the read-only backlog has not moved (D108.a)
//!
//! Hand-authored only: the hold stays until [`super::ReleaseBacklog`], and a
//! random reboot drawn before it would wait for a backlog that cannot drain.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the app boots again while its read-only files cannot be read yet")]
pub struct RebootWithBacklogHeld;

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionFactory<R> for RebootWithBacklogHeld {
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

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionRef<R> for RebootWithBacklogHeld {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        crate::pbt::transitions::Reboot.preconditions(state)
    }

    /// The store keeps the recipe from the boot before, so the model has no
    /// pending state: a held backlog only delays what is already there.
    fn apply_to_ref(&self, state: &mut R) {
        state.reboot_drops_in_memory_state();
    }
}

crate::cap_transition! {
    RebootWithBacklogHeld: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout + RefReboot ],
    |_me, _state, sut| {
        // The composed harness intercepts this like `Reboot`.
        sut.reboot_with_backlog_held().await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + crate::pbt::transition_budgets::docs_tolerance(state),
        }
    }
}
