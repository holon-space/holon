//! Transition: edit a block on a peer's LoroDoc directly.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1481-1534` (generator),
//! `state_machine.rs:3494-3511` (precondition),
//! `state_machine.rs:2788-2818` (ref-state apply),
//! `sut.rs:4392-4418` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::transitions::{PeerEditOp, deterministic_peer_block_id};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Edit a block on a peer's LoroDoc directly (no SQL, no BackendEngine).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerEdit {
    pub peer_idx: usize,
    pub op: PeerEditOp,
}

impl E2ETransitionFactory for PeerEdit {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Early gate: app started, Loro enabled, peers available.
        // `preconditions` checks peer_idx bounds and op validity.
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.variant.enable_loro, Reason::LoroRequiredForPeers),
            check(!state.peers.is_empty(), Reason::NoPeersAvailable),
        ];
        let merged: Validated<Vec<()>, Reason> = checks.into_iter().collect();
        if merged.is_fail() {
            return merged.map(|_| unreachable!());
        }

        (|| {
            let peer_count = state.peers.len();
            let seq = state.block_state.next_id;

            let mut arms: Vec<(u32, BoxedStrategy<PeerEdit>)> = Vec::new();

            // PeerEdit::Create — deterministic stable ID from hash of
            // (peer_idx, parent, content, seq) ensures ref model and SUT agree.
            {
                // Extract peer data before the strategy to avoid borrow escaping.
                let peer_blocks_per_idx: Vec<Vec<String>> = (0..peer_count)
                    .map(|idx| state.peers[idx].blocks.keys().cloned().collect::<Vec<_>>())
                    .collect();

                let pc = peer_count;
                let create = (0..pc, "[a-z]{4,8}")
                    .prop_flat_map(move |(peer_idx, content)| {
                        let has_blocks = !peer_blocks_per_idx[peer_idx].is_empty();
                        let parent_strat = if has_blocks {
                            proptest::option::of(proptest::sample::select(
                                peer_blocks_per_idx[peer_idx].clone(),
                            ))
                            .boxed()
                        } else {
                            Just(None).boxed()
                        };
                        parent_strat.prop_map(move |parent_stable_id| {
                            let sid = deterministic_peer_block_id(
                                peer_idx,
                                parent_stable_id.as_deref(),
                                &content,
                                seq,
                            );
                            PeerEdit {
                                peer_idx,
                                op: PeerEditOp::Create {
                                    parent_stable_id,
                                    content: content.clone(),
                                    stable_id: sid,
                                },
                            }
                        })
                    })
                    .boxed();
                arms.push((1, create));
            }

            // PeerEdit::Delete is disabled: cascading-delete ref model gap.
            //
            // PeerEdit::Update — enumerate all (peer_idx, stable_id) pairs
            // and filter via preconditions (which checks source-block exclusion).
            {
                let all_peers: Vec<(usize, Vec<String>)> = (0..peer_count)
                    .map(|idx| {
                        let ids = state.peers[idx].blocks.keys().cloned().collect::<Vec<_>>();
                        (idx, ids)
                    })
                    .filter(|(_, ids)| !ids.is_empty())
                    .collect();

                if !all_peers.is_empty() {
                    let update = proptest::sample::select(all_peers)
                        .prop_flat_map(|(peer_idx, ids)| {
                            (Just(peer_idx), proptest::sample::select(ids), "[a-z]{4,8}")
                        })
                        .prop_map(|(peer_idx, stable_id, content)| PeerEdit {
                            peer_idx,
                            op: PeerEditOp::Update { stable_id, content },
                        })
                        .boxed();
                    arms.push((1, update));
                }
            }

            if arms.is_empty() {
                return Validated::fail(Reason::NoPeersAvailable);
            }

            let strat = Union::new_weighted(arms).boxed();
            Validated::Good((1, strat))
        })()
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for PeerEdit {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(
                self.peer_idx < state.peers.len(),
                Reason::PeerIndexOutOfBounds,
            ),
        ];

        if self.peer_idx < state.peers.len() {
            let peer = &state.peers[self.peer_idx];
            let valid_op = match &self.op {
                PeerEditOp::Create {
                    parent_stable_id, ..
                } => parent_stable_id
                    .as_ref()
                    .is_none_or(|pid| peer.blocks.contains_key(pid)),
                PeerEditOp::Update { stable_id, .. } => peer.blocks.contains_key(stable_id),
                PeerEditOp::Delete { stable_id } => peer.blocks.contains_key(stable_id),
            };

            // Use PeerEditSourceBlockViolation for source-block exclusions if needed
            checks.push(check(valid_op, Reason::PeerEditSourceBlockViolation));
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use holon_pbt_core::capabilities::RefPeersMut;
        match &self.op {
            PeerEditOp::Create {
                parent_stable_id,
                content,
                stable_id,
            } => state.peer_apply_create(
                self.peer_idx,
                parent_stable_id.as_deref(),
                content,
                stable_id,
            ),
            PeerEditOp::Update { stable_id, content } => {
                state.peer_apply_update(self.peer_idx, stable_id, content)
            }
            PeerEditOp::Delete { stable_id } => state.peer_apply_delete(self.peer_idx, stable_id),
        }
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_peer_edit(self.peer_idx, &self.op).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        // PeerEdit: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
