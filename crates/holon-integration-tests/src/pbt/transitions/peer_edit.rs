//! Transition: edit a block on a peer's LoroDoc directly.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1481-1534` (generator),
//! `state_machine.rs:3494-3511` (precondition),
//! `state_machine.rs:2788-2818` (ref-state apply),
//! `sut.rs:4392-4418` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};

use super::E2ETransitionImpl;
use crate::pbt::peer_ops::PeerBlock;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::transitions::{PeerEditOp, deterministic_peer_block_id};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

use holon_api::ContentType;

/// Edit a block on a peer's LoroDoc directly (no SQL, no BackendEngine).
#[derive(Clone, Debug)]
pub struct PeerEdit {
    pub peer_idx: usize,
    pub op: PeerEditOp,
}

impl E2ETransitionFactory for PeerEdit {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started || !state.variant.enable_loro {
            return None;
        }
        if state.peers.is_empty() {
            return None;
        }
        let peer_count = state.peers.len();

        // Source blocks cannot have child blocks in org syntax — the
        // OrgRenderer would re-parent any children to the source's
        // parent on the next sync, diverging from the reference model.
        // Exclude source blocks from valid create parents.
        let source_block_ids: std::collections::HashSet<String> = state
            .block_state
            .blocks
            .values()
            .filter(|b| b.content_type == ContentType::Source)
            .map(|b| b.id.id().to_string())
            .collect();

        // Collect stable IDs from all peers for update/delete/nested-create targets
        let all_peer_block_ids: Vec<(usize, Vec<String>)> = state
            .peers
            .iter()
            .enumerate()
            .map(|(idx, p)| (idx, p.blocks.keys().cloned().collect::<Vec<_>>()))
            .collect();

        // Peers with non-source blocks (for valid create-parent candidates).
        let peers_with_nonsource_blocks: Vec<(usize, Vec<String>)> = all_peer_block_ids
            .iter()
            .map(|(idx, ids)| {
                let filtered: Vec<String> = ids
                    .iter()
                    .filter(|id| !source_block_ids.contains(*id))
                    .cloned()
                    .collect();
                (*idx, filtered)
            })
            .filter(|(_, ids)| !ids.is_empty())
            .collect();

        let seq = state.block_state.next_id;

        let mut arms: Vec<(u32, BoxedStrategy<PeerEdit>)> = Vec::new();

        // PeerEdit::Create — deterministic stable ID from hash of
        // (peer_idx, parent, content, seq) ensures ref model and SUT agree.
        {
            let pc = peer_count;
            let pwb_for_create = peers_with_nonsource_blocks.clone();
            let create = (0..pc, "[a-z]{4,8}")
                .prop_flat_map(move |(peer_idx, content)| {
                    let parent = if let Some((_, ids)) =
                        pwb_for_create.iter().find(|(i, _)| *i == peer_idx)
                    {
                        proptest::option::of(proptest::sample::select(ids.clone())).boxed()
                    } else {
                        Just(None).boxed()
                    };
                    parent.prop_map(move |parent_stable_id| {
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
        // PeerEdit::Update — update content of an existing peer block.
        // Source blocks are excluded: peer content is random `[a-z]{4,8}`
        // which would not be valid GQL/PRQL/SQL/YAML, breaking the root
        // layout's query parse and tripping inv10.
        if !peers_with_nonsource_blocks.is_empty() {
            let pwb = peers_with_nonsource_blocks.clone();
            let update = proptest::sample::select(pwb)
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

        if arms.is_empty() {
            return None;
        }

        let strat = Union::new_weighted(arms).boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for PeerEdit {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        if !state.app_started || self.peer_idx >= state.peers.len() {
            return false;
        }
        let peer = &state.peers[self.peer_idx];
        match &self.op {
            PeerEditOp::Create {
                parent_stable_id, ..
            } => parent_stable_id
                .as_ref()
                .is_none_or(|pid| peer.blocks.contains_key(pid)),
            PeerEditOp::Update { stable_id, .. } => peer.blocks.contains_key(stable_id),
            PeerEditOp::Delete { stable_id } => peer.blocks.contains_key(stable_id),
        }
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let peer = &mut state.peers[self.peer_idx];
        match &self.op {
            PeerEditOp::Create {
                parent_stable_id,
                content,
                stable_id,
            } => {
                peer.blocks.insert(
                    stable_id.clone(),
                    PeerBlock {
                        stable_id: stable_id.clone(),
                        parent_stable_id: parent_stable_id.clone(),
                        content: content.clone(),
                    },
                );
                peer.created_stable_ids.insert(stable_id.clone());
            }
            PeerEditOp::Update { stable_id, content } => {
                if let Some(block) = peer.blocks.get_mut(stable_id) {
                    block.content = content.clone();
                    peer.modified_stable_ids.insert(stable_id.clone());
                }
            }
            PeerEditOp::Delete { stable_id } => {
                peer.blocks.remove(stable_id);
                peer.deleted_stable_ids.insert(stable_id.clone());
            }
        }
    }

    async fn apply_to_sut(&self, _ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_peer_edit(self.peer_idx, &self.op).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        // PeerEdit: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
