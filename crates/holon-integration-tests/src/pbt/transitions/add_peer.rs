//! Transition: add a Loro-only peer instance.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1431-1433` (generator),
//! `state_machine.rs:3491-3492` (precondition),
//! `state_machine.rs:2738-2786` (ref-state apply),
//! `sut.rs:4365-4390` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::peer_ops::PeerBlock;
use crate::pbt::reference_state::{PeerRefState, ReferenceState};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Add a Loro-only peer instance that shares the primary's current state.
#[derive(Clone, Debug)]
pub struct AddPeer;

impl E2ETransitionFactory for AddPeer {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started || !state.variant.enable_loro {
            return None;
        }
        if state.peers.len() >= 3 {
            return None;
        }
        Some((1, Just(AddPeer).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for AddPeer {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && state.variant.enable_loro && state.peers.len() < 3
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let peer_id = (state.peers.len() as u64) + 100;
        // Peer starts with a copy of the primary's blocks.
        // Exclude seed blocks (doc = sentinel) — they are inserted via
        // direct SQL and never reverse-synced to Loro, so the actual
        // peer LoroDoc (created from a Loro snapshot) won't have them.
        let peer_blocks: std::collections::HashMap<String, PeerBlock> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                // Exclude seed blocks (doc = sentinel/no_parent).
                let is_seed = state
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                // Exclude document blocks (name != None) — they exist
                // in the reference model but not as Loro tree nodes.
                !is_seed && !b.is_page()
            })
            .map(|b| {
                let pb = PeerBlock {
                    stable_id: b.id.id().to_string(),
                    parent_stable_id: if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                        None
                    } else {
                        Some(b.parent_id.id().to_string())
                    },
                    content: b.content_text().to_string(),
                };
                (pb.stable_id.clone(), pb)
            })
            .collect();
        let baseline_contents = peer_blocks
            .values()
            .map(|pb| (pb.stable_id.clone(), pb.content.clone()))
            .collect();
        state.peers.push(PeerRefState {
            peer_id,
            blocks: peer_blocks,
            deleted_stable_ids: std::collections::HashSet::new(),
            modified_stable_ids: std::collections::HashSet::new(),
            created_stable_ids: std::collections::HashSet::new(),
            baseline_contents,
        });
    }

    async fn apply_to_sut(&self, _ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_add_peer().await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        // AddPeer: export_snapshot triggers ~5 SQL reads (store persistence).
        // Others: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
