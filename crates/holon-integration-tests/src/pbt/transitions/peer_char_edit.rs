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
use validated::Validated;

use holon_pbt_core::capabilities::TextOp;
use holon_pbt_core::capabilities::{RefLifecycle, RefPeers};
use holon_pbt_core::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Edit a block's LoroText container on a peer at the character level.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerCharEdit {
    pub peer_idx: usize,
    pub block_id: String,
    pub op: TextOp,
}

impl<R: RefLifecycle + RefPeers> TransitionFactory<R> for PeerCharEdit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // The legacy generator never emits PeerCharEdit — it has no
        // `strategies.add_weighted("peer_char_edit", ...)` call. Mirror that
        // exactly: return None so this variant is never selected.
        Validated::fail(Reason::Unmigrated)
    }
}

impl<R: RefLifecycle + RefPeers> TransitionRef<R> for PeerCharEdit {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.mutable_text_enabled(), Reason::Unmigrated),
            check(state.app_started(), Reason::AppNotStarted),
            check(
                self.peer_idx < state.peers_len(),
                Reason::PeerIndexOutOfBounds,
            ),
        ];

        if self.peer_idx < state.peers_len() {
            checks.push(check(
                state
                    .peer_block_content(self.peer_idx, &self.block_id)
                    .is_some(),
                Reason::PeerBlockMissing,
            ));
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // Reference model: PeerCharEdit doesn't change block-level
        // content (it operates at the LoroText character level).
        // The block content in the reference model stays the same;
        // cross-peer text convergence is checked by inv-cross-peer-
        // text-convergence after SyncWithPeer.
        let _ = (&self.peer_idx, &self.block_id);
    }
}

crate::cap_transition! {
    PeerCharEdit: holon_pbt_core::capabilities::SutLoro,
    where R: [ RefLifecycle + RefPeers ],
    |me, _state, sut| {
        sut.apply_peer_char_edit(me.peer_idx, &me.block_id, &me.op)
            .await;
    }
    sql_budget: |_me, _state| {
        // PeerCharEdit: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
