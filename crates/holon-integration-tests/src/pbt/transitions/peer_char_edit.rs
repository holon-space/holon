//! Transition: edit a block's LoroText container on a peer at character level.
//!
//! Mirrors the legacy logic split across (no generator — never generated),
//! `state_machine.rs:3517-3524` (precondition),
//! `state_machine.rs:2921-2932` (ref-state apply),
//! `sut.rs:4420-4452` (SUT apply), and
//! `transition_budgets.rs:353-360` (expected SQL).
//!
//! Note: the legacy generator never emits this transition — it is defined but
//! not added to any weighted strategy. `weighted_generator` therefore returns
//! `None` unconditionally, matching that behaviour exactly.

use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::transitions::TextOp;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Edit a block's LoroText container on a peer at the character level.
#[derive(Clone, Debug)]
pub struct PeerCharEdit {
    pub peer_idx: usize,
    pub block_id: String,
    pub op: TextOp,
}

impl E2ETransitionFactory for PeerCharEdit {
    fn weighted_generator(_state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        // The legacy generator never emits PeerCharEdit — it has no
        // `strategies.add_weighted("peer_char_edit", ...)` call. Mirror that
        // exactly: return None so this variant is never selected.
        None
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for PeerCharEdit {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::mutable_text_enabled()
            && state.app_started
            && self.peer_idx < state.peers.len()
            && state.peers[self.peer_idx]
                .blocks
                .contains_key(&self.block_id)
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // Reference model: PeerCharEdit doesn't change block-level
        // content (it operates at the LoroText character level).
        // The block content in the reference model stays the same;
        // cross-peer text convergence is checked by inv-cross-peer-
        // text-convergence after SyncWithPeer.
        let _ = (&self.peer_idx, &self.block_id);
    }

    async fn apply_to_sut(&self, _ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_peer_char_edit(self.peer_idx, &self.block_id, &self.op)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        // PeerCharEdit: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
