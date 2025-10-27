//! Transition: remove an active watch (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:534-542` (generator),
//! `state_machine.rs:3161-3163` (precondition),
//! `state_machine.rs:2216-2218` (ref-state apply),
//! `sut.rs:1319-1321` (SUT apply), and
//! `transition_budgets.rs:144-150` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::SutWatchRegister;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

/// Remove an active query watch.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RemoveWatch {
    pub query_id: String,
}

impl TransitionFactory<ReferenceState> for RemoveWatch {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutWatchRegister,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the navigation / CDC-watch / MCP providers this transition
        // dispatches have no Loro-native source in the no-Turso wiring
        // (see loro_block_query_source.rs:77). Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (active watch IDs) and let
        // `preconditions` be the single source of truth for which ones are
        // actually removable. Avoids duplicating the app_started / watch_exists checks.
        let candidates: Vec<String> = state
            .mcp
            .active_watches
            .keys()
            .filter(|query_id| {
                RemoveWatch {
                    query_id: query_id.to_string(),
                }
                .preconditions(state)
                .is_good()
            })
            .cloned()
            .collect();
        check(!candidates.is_empty(), Reason::NoActiveWatches).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|query_id| RemoveWatch { query_id })
                .boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for RemoveWatch {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(
                state.mcp.active_watches.contains_key(&self.query_id),
                Reason::NoActiveWatches,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.mcp.active_watches.remove(&self.query_id);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutWatchRegister> TransitionImpl<ReferenceState, S> for RemoveWatch {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.unregister_watch(&self.query_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for RemoveWatch {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
