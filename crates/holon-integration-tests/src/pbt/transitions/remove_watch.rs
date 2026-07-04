//! Transition: remove an active watch (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:534-542` (generator),
//! `state_machine.rs:3161-3163` (precondition),
//! `state_machine.rs:2216-2218` (ref-state apply),
//! `sut.rs:1319-1321` (SUT apply), and
//! `transition_budgets.rs:144-150` (expected SQL).

use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{RefLifecycle, RefWatch, RefWatchesMut, SutWatchRegister};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

/// Remove an active query watch.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RemoveWatch {
    pub query_id: String,
}

impl<R: RefLifecycle + RefWatch + RefWatchesMut> TransitionFactory<R> for RemoveWatch {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: the navigation / CDC-watch / MCP providers this transition
        // dispatches have no Loro-native source in the no-Turso wiring
        // (see loro_block_query_source.rs:77). Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (active watch IDs) and let
        // `preconditions` be the single source of truth for which ones are
        // actually removable. Avoids duplicating the app_started / watch_exists checks.
        let candidates: Vec<String> = state
            .active_watch_ids()
            .into_iter()
            .filter(|query_id| {
                RemoveWatch {
                    query_id: query_id.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::NoActiveWatches).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|query_id| RemoveWatch { query_id })
                .boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefWatch + RefWatchesMut> TransitionRef<R> for RemoveWatch {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.active_watch_ids().contains(&self.query_id),
                Reason::NoActiveWatches,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.remove_watch(&self.query_id);
    }
}

crate::cap_transition! {
    RemoveWatch: SutWatchRegister,
    where R: [ RefLifecycle + RefWatch + RefWatchesMut ],
    |me, _state, sut| {
        sut.unregister_watch(&me.query_id).await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}
