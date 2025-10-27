//! Transition: attempt a consolidator epoch flip and assert it is rejected.
//!
//! Spec 0008 §4.2(b): with the app already running, a SECOND in-process boot over
//! the SAME vault/db/config paths but with the consolidator flipped (Loro ⇄ SQL)
//! must fail with Model.md invariant 10's hard error, fired through the REAL boot
//! path (`holon_app::new_from_config_with_di` → wiring guard) BEFORE any higher
//! store opens — the live app and the persisted epoch marker stay untouched.
//!
//! Mid-run, `SutAppLifecycle`-bound, `ref_state`-free (mirrors `SimulateRestart`):
//! the assertion lives inside the SUT apply (`assert_epoch_flip_rejected`), not a
//! new invariant — the transition IS the teeth. Turso-gated because the flip needs
//! a durable db so the epoch marker genuinely exists (the SUT method fails loud if
//! it selected without one).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::local_caps::SutAppLifecycle;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Attempt to boot a second consolidator over the running app's durable state and
/// assert the invariant-10 epoch guard rejects it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EpochFlipRejected;

impl TransitionFactory<ReferenceState> for EpochFlipRejected {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn crate::pbt::local_caps::SutAppLifecycle,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the flip re-boots over the on-disk Turso db whose durable
        // footprint makes the epoch guard write (and then protect) the marker.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        EpochFlipRejected
            .preconditions(state)
            // Low weight (like `SimulateRestart`): a rare mid-run epoch-flip probe.
            .map(|()| (1, Just(EpochFlipRejected).boxed()))
    }
}

impl TransitionRef<ReferenceState> for EpochFlipRejected {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        // App must be running: the flip re-boots over the live app's own paths.
        check(state.action.app_started, Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // A REJECTED boot changes nothing: the live app and its durable state are
        // untouched, so the reference state is unchanged.
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutAppLifecycle> TransitionImpl<ReferenceState, S> for EpochFlipRejected {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.assert_epoch_flip_rejected().await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for EpochFlipRejected {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        // The flip dies at the wiring guard BEFORE `BackendEngine`/matview/CDC
        // resolution, so no SQL flows through the LIVE (traced) engine. The
        // transient second Turso connection `open_and_register_core` opens ahead of
        // the guard is a separate, untraced handle. Small tolerance in case an
        // open-time PRAGMA is ever attributed to the traced engine; if the real
        // attempt is observed issuing SQL here, that itself is worth reporting.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4,
        }
    }
}
