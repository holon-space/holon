//! Transition: trigger IVM re-evaluation to detect CDC re-emission bugs.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1424-1427` (generator),
//! `state_machine.rs:3490` (precondition),
//! `state_machine.rs:2733-2735` (ref-state apply),
//! `sut.rs:4169-4176` (SUT apply), and
//! `transition_budgets.rs:331-336` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, docs_tolerance};

/// Trigger IVM re-evaluation to detect CDC re-emission bugs.
/// Generated with low weight — useful after navigation transitions.
#[derive(Clone, Debug)]
pub struct EmitMcpData;

impl E2ETransitionFactory for EmitMcpData {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        Some((3, Just(EmitMcpData).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for EmitMcpData {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // No reference state change — just triggers IVM re-evaluation.
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_emit_mcp_data().await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let blocks = state.block_state.blocks.len();
        ExpectedSql {
            reads: 4,
            writes: 2,
            ddl: 0,
            tolerance: docs_tolerance(state) + blocks * 6,
        }
    }
}
