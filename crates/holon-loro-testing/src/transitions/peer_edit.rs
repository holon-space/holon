//! Transition: edit a block on a peer's LoroDoc directly.
//!
//! @pbt rung external
//!   edits a block on a peer's LoroDoc directly (CRDT stimulus).
//! @pbt covers loro-peer-edit — peer-side block mutation on LoroDoc
//!
//! Mirrors the legacy logic split across `state_machine.rs:1481-1534`
//! (generator), `state_machine.rs:3494-3511` (precondition),
//! `state_machine.rs:2788-2818` (ref-state apply),
//! `sut.rs:4392-4418` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
#[cfg(feature = "otel-testing")]
use holon_pbt_core::budget::ExpectedSql;
use holon_pbt_core::capabilities::PeerEditOp;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;
use holon_pbt_core::capabilities::deterministic_peer_block_id;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

/// Edit a block on a peer's LoroDoc directly (no SQL, no BackendEngine).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerEdit {
    pub peer_idx: usize,
    pub op: PeerEditOp,
}

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionFactory<R> for PeerEdit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Early gate: app started, Loro enabled, peers available.
        // `preconditions` checks peer_idx bounds and op validity.
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.enable_loro(), Reason::LoroRequiredForPeers),
            check(state.peers_len() > 0, Reason::NoPeersAvailable),
        ];
        let merged: Validated<Vec<()>, Reason> = checks.into_iter().collect();
        if merged.is_fail() {
            return merged.map(|_| unreachable!());
        }

        (|| {
            let peer_count = state.peers_len();
            let seq = state.next_block_id();

            let mut arms: Vec<(u32, BoxedStrategy<PeerEdit>)> = Vec::new();

            // PeerEdit::Create — deterministic stable ID from hash of
            // (peer_idx, parent, content, seq) ensures ref model and SUT agree.
            {
                // Extract peer data before the strategy to avoid borrow escaping.
                let peer_blocks_per_idx: Vec<Vec<String>> = (0..peer_count)
                    .map(|idx| state.peer_block_stable_ids(idx))
                    .collect();

                let pc = peer_count;
                let create = (
                    0..pc,
                    holon_pbt_core::content_generators::peer_content_strategy(),
                )
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
                    .map(|idx| (idx, state.peer_block_stable_ids(idx)))
                    .filter(|(_, ids)| !ids.is_empty())
                    .collect();

                if !all_peers.is_empty() {
                    let update = proptest::sample::select(all_peers)
                        .prop_flat_map(|(peer_idx, ids)| {
                            (
                                Just(peer_idx),
                                proptest::sample::select(ids),
                                holon_pbt_core::content_generators::peer_content_strategy(),
                            )
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

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionRef<R> for PeerEdit {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                self.peer_idx < state.peers_len(),
                Reason::PeerIndexOutOfBounds,
            ),
        ];

        if self.peer_idx < state.peers_len() {
            let has_block = |sid: &str| state.peer_block_content(self.peer_idx, sid).is_some();
            let valid_op = match &self.op {
                PeerEditOp::Create {
                    parent_stable_id, ..
                } => parent_stable_id.as_ref().is_none_or(|pid| has_block(pid)),
                PeerEditOp::Update { stable_id, .. } => has_block(stable_id),
                PeerEditOp::Delete { stable_id } => has_block(stable_id),
            };

            // Use PeerEditSourceBlockViolation for source-block exclusions if needed
            checks.push(check(valid_op, Reason::PeerEditSourceBlockViolation));
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
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
}

holon_pbt_core::cap_transition! {
    PeerEdit: holon_pbt_core::capabilities::SutLoro,
    where R: [ RefLifecycle + RefPeers + RefPeersMut ],
    |me, _state, sut| {
        sut.apply_peer_edit(me.peer_idx, &me.op).await;
    }
    sql_budget: |_me, _state| {
        // PeerEdit: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
