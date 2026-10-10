//! Transition: reboot into a binary with another scalar-function set.
//!
//! @pbt rung external
//!   records a foreign function set in the retained database while no session
//!   holds it, then boots again over the same store
//!   (`SutAppLifecycle::reboot_after_upgrade`).
//! @pbt covers upgrade-keeps-the-database — an upgrade changes nothing a user
//!   can see: every row the database held survives, in both storage modes

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// The reference model is [`super::Reboot`]'s: the views over the store are
/// rebuilt, the store itself is not.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the app is upgraded and rebooted")]
pub struct RebootAfterUpgrade;

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionFactory<R> for RebootAfterUpgrade {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    /// Gated with [`super::Reboot`] (`HOLON_PBT_REBOOT=1`), for its reason.
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let enabled = std::env::var("HOLON_PBT_REBOOT").is_ok();
        let checks: Vec<Validated<(), Reason>> = vec![
            check(enabled, Reason::PreconditionFailed),
            RebootAfterUpgrade.preconditions(state),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                (
                    super::reboot::reboot_weight(),
                    Just(RebootAfterUpgrade).boxed(),
                )
            })
    }
}

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionRef<R> for RebootAfterUpgrade {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        crate::pbt::transitions::Reboot.preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.reboot_drops_in_memory_state();
    }
}

crate::cap_transition! {
    RebootAfterUpgrade: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout + RefReboot ],
    |_me, _state, sut| {
        // The composed harness intercepts this like `Reboot`.
        sut.reboot_after_upgrade().await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + docs_tolerance(state),
        }
    }
}
