//! Transition: the other process lets go of the database, and the app restarts
//! onto it.
//!
//! @pbt rung external
//!   drops the `held_lock_seam` hold, waits until the session says the
//!   database is free, and boots again
//!   (`SutAppLifecycle::release_database_and_restart`).
//! @pbt covers boot-with-a-held-database — the restart after the release opens
//!   the real database and the held condition ends
//!
//! Hand-authored only, after [`super::RebootWhileDatabaseHeld`].

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the database is free again and the app restarts")]
pub struct ReleaseDatabaseAndRestart;

impl TransitionFactory<ReferenceState> for ReleaseDatabaseAndRestart {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        super::RebootWhileDatabaseHeld::required_wiring()
    }

    fn weighted_generator(_: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl TransitionRef<ReferenceState> for ReleaseDatabaseAndRestart {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        crate::pbt::transitions::Reboot.preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.reboot_drops_in_memory_state();
        state.ui.boot_on_released_database();
        state.conditions.clear(
            holon_api::condition_bus::DATABASE_SUBJECT,
            holon_api::ConditionKind::DATABASE_HELD,
        );
    }
}

crate::cap_transition! {
    ReleaseDatabaseAndRestart: SutAppLifecycle,
    where R: [ RefReboot ],
    |_me, _state, sut| {
        // The composed harness intercepts this like `Reboot`.
        sut.release_database_and_restart().await;
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
