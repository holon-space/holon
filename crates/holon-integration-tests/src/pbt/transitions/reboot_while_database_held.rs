//! Transition: reboot while another process holds the database file.
//!
//! @pbt rung external
//!   holds `test.db` through `holon_turso::db_open::held_lock_seam` across the
//!   shutdown and boots again (`SutAppLifecycle::reboot_while_database_held`).
//! @pbt covers boot-with-a-held-database — the boot projects into memory and
//!   discloses the held database
//!
//! Hand-authored only: the hold stays until
//! [`super::ReleaseDatabaseAndRestart`].

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
#[step_template("the app boots again while another process holds its database")]
pub struct RebootWhileDatabaseHeld;

impl TransitionFactory<ReferenceState> for RebootWhileDatabaseHeld {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    /// Only a boot that projects through Loro may run on memory; without it
    /// the database is the store and the boot waits for its lock.
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::All(vec![
            ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso),
            ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro),
        ])
    }

    fn weighted_generator(_: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl TransitionRef<ReferenceState> for RebootWhileDatabaseHeld {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        crate::pbt::transitions::Reboot.preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.reboot_drops_in_memory_state();
        state.ui.boot_on_memory_while_database_held();
        state.conditions.raise_with_message(
            holon_api::condition_bus::DATABASE_SUBJECT,
            holon_api::ConditionKind::DATABASE_HELD,
            database_hold_message(holon_api::DatabaseHold::Held),
        );
        // The probe answers every second; a failed one would say so here.
        state
            .conditions
            .clear_kind(holon_api::ConditionKind::DATABASE_WATCH_STOPPED);
    }
}

/// The message the keystone reads a `database-held` condition's hold as.
pub fn database_hold_message(hold: holon_api::DatabaseHold) -> String {
    format!("{hold:?}")
}

crate::cap_transition! {
    RebootWhileDatabaseHeld: SutAppLifecycle,
    where R: [ RefReboot ],
    |_me, _state, sut| {
        // The composed harness intercepts this like `Reboot`.
        sut.reboot_while_database_held().await;
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
