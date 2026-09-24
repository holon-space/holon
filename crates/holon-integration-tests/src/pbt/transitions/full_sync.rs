//! Transition: run `*::full_sync`, which drops every Turso watch view and
//! recreates the ones a live watch listens to.
//!
//! @pbt rung mcp
//!   drives the MCP `execute_operation` tool, the entry point an agent uses;
//!   no UI binding reaches `full_sync`.
//! @pbt covers watch-views-survive-full-sync — a watch opened before a
//!   `full_sync` keeps receiving changes after it

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutFullSync;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an agent runs a full sync")]
pub struct FullSync;

impl<R: RefLifecycle> TransitionFactory<R> for FullSync {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        FullSync
            .preconditions(state)
            .map(|_| (1, Just(FullSync).boxed()))
    }
}

impl<R: RefLifecycle> TransitionRef<R> for FullSync {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut R) {
        // No provider in the keystone holds a sync cache or token, so the
        // model state does not change: only the views are rebuilt.
    }
}

crate::cap_transition! {
    FullSync: SutFullSync,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.full_sync().await;
    }
    sql_budget: |_me, state| {
        // Per watch view: its drop, and for a listened one the recreate with
        // its existence probes and DBSP-state cleanup. The reference does not
        // model how many watch views exist; the rendered blocks and documents
        // bound them. Writes: the sync-token reset.
        let views = state.block_count() + state.document_count();
        ExpectedSql {
            reads: 4 * views,
            writes: 2,
            ddl: 2 * views,
            tolerance: docs_tolerance(state),
        }
    }
}
