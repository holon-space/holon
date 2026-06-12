//! Transition: trigger IVM re-evaluation to detect CDC re-emission bugs.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1424-1427` (generator),
//! `state_machine.rs:3490` (precondition),
//! `state_machine.rs:2733-2735` (ref-state apply),
//! `sut.rs:4169-4176` (SUT apply), and
//! `transition_budgets.rs:331-336` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::SutMcpEmit;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, docs_tolerance};

/// Trigger IVM re-evaluation to detect CDC re-emission bugs.
/// Generated with low weight — useful after navigation transitions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EmitMcpData;

impl TransitionFactory<ReferenceState> for EmitMcpData {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutMcpEmit,
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
        // Delegate all validation to preconditions — single source of truth.
        EmitMcpData
            .preconditions(state)
            .map(|_| (3, Just(EmitMcpData).boxed()))
    }
}

impl TransitionRef<ReferenceState> for EmitMcpData {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.action.app_started, Reason::AppNotStarted)];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // No reference state change — just triggers IVM re-evaluation.
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutMcpEmit> TransitionImpl<ReferenceState, S> for EmitMcpData {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.emit_mcp_data().await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for EmitMcpData {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let blocks = state.domain.block_state.blocks.len();
        ExpectedSql {
            reads: 4,
            writes: 2,
            ddl: 0,
            tolerance: docs_tolerance(state) + blocks * 6,
        }
    }
}
