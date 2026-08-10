//! Transition: trigger IVM re-evaluation to detect CDC re-emission bugs.
//!
//! @pbt rung mcp
//!   purpose is an MCP re-emission; but the headless frontend has no
//!   PbtMcpIntegration attached -> `emit_mcp_data` is a faithful no-op AND no
//!   invariant observes an emission on this path (see audit finding TR-OBS).
//! @pbt covers mcp-ivm-reemission — CDC re-eval to detect duplicate MCP
//! emission
//!
//! Mirrors the legacy logic split across `state_machine.rs:1424-1427`
//! (generator), `state_machine.rs:3490` (precondition),
//! `state_machine.rs:2733-2735` (ref-state apply),
//! `sut.rs:4169-4176` (SUT apply), and
//! `transition_budgets.rs:331-336` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutMcpEmit;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Trigger IVM re-evaluation to detect CDC re-emission bugs.
/// Generated with low weight — useful after navigation transitions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the MCP emits its data")]
pub struct EmitMcpData;

impl<R: RefLifecycle> TransitionFactory<R> for EmitMcpData {
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
        // Delegate all validation to preconditions — single source of truth.
        EmitMcpData
            .preconditions(state)
            .map(|_| (3, Just(EmitMcpData).boxed()))
    }
}

impl<R: RefLifecycle> TransitionRef<R> for EmitMcpData {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started(), Reason::AppNotStarted)];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // No reference state change — just triggers IVM re-evaluation.
    }
}

crate::cap_transition! {
    EmitMcpData: SutMcpEmit,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.emit_mcp_data().await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        ExpectedSql {
            reads: 4,
            writes: 2,
            ddl: 0,
            tolerance: docs_tolerance(state) + blocks * 6,
        }
    }
}
