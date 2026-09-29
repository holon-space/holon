//! Transition: attempt a consolidator epoch flip and assert it is rejected.
//!
//! @pbt rung dispatch
//!   `assert_epoch_flip_rejected` is a consolidator-level assertion probe.
//! @pbt covers consolidator-epoch-flip-reject — stale epoch flip must be
//! rejected
//!
//! Spec 0008 §4.2(b): the live session shuts down, a boot over the SAME
//! vault/db/config paths but with the consolidator flipped (Loro ⇄ SQL) must
//! fail with Model.md invariant 10's hard error, fired through the REAL boot
//! path (`holon_app::new_from_config_with_di` → wiring guard) and leaving the
//! epoch marker untouched, then the live session boots again as in `Reboot`.
//! The shutdown comes first because a second session on a held vault is
//! refused by the writer lock before the guard runs (`SecondWriterRefused`).
//!
//! Turso-gated because the flip needs a durable db so the epoch marker
//! genuinely exists (the SUT method fails loud if it selected without one).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Reboot the app, attempting a flipped-consolidator boot in between, and
/// assert the invariant-10 epoch guard rejects it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an epoch flip is rejected")]
pub struct EpochFlipRejected;

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionFactory<R> for EpochFlipRejected {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the flip re-boots over the on-disk Turso db whose durable
        // footprint makes the epoch guard write (and then protect) the marker.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        EpochFlipRejected
            .preconditions(state)
            // Low weight (like `SimulateRestart`): a rare mid-run epoch-flip probe.
            .map(|()| (1, Just(EpochFlipRejected).boxed()))
    }
}

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionRef<R> for EpochFlipRejected {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        crate::pbt::transitions::Reboot.preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The rejected boot changes nothing; the reboot around it is a `Reboot`.
        state.reboot_drops_in_memory_state();
    }
}

crate::cap_transition! {
    EpochFlipRejected: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout + RefReboot ],
    |_me, _state, sut| {
        // The composed harness intercepts this like `Reboot`
        // (`ComposedSlice::is_reboot`); this arm serves the non-composed ones.
        sut.assert_epoch_flip_rejected().await;
    }
    sql_budget: |_me, state| {
        // The window opens after the reboot, so only `Reboot`'s bookkeeping is
        // inside it.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + crate::pbt::transition_budgets::docs_tolerance(state),
        }
    }
}
