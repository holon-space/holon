//! Shared multi-peer sync infrastructure for property-based testing.
//!
//! Provides `PeerState`, `GroupState`, `GroupTransition`, and helpers for
//! generating, applying, and checking multi-peer Loro sync scenarios.
//! Used by both `sync_pbt` (unit-level) and `general_e2e_composed_pbt`
//! (integration).

use std::collections::HashSet;
use std::sync::Arc;

use loro::Container;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroText;
use loro::LoroTree;
use loro::TreeID;
use loro::ValueOrContainer;
use proptest::prelude::*;

pub use crate::loro_backend::CONTENT_RAW;
pub use crate::loro_backend::CONTENT_TYPE;
pub use crate::loro_backend::SOURCE_CODE;
pub use crate::loro_backend::STABLE_ID;
pub use crate::loro_backend::TREE_NAME;

/// Return the metadata field that stores a node's primary text content,
/// based on its `content_type` (`source` blocks live in `source_code`,
/// everything else lives in `content_raw`).
pub fn content_field_for(meta: &loro::LoroMap) -> &'static str {
    let is_source = matches!(
        meta.get(CONTENT_TYPE),
        Some(ValueOrContainer::Value(ref v)) if v.as_string().map(|s| s.as_str()) == Some("source")
    );
    if is_source { SOURCE_CODE } else { CONTENT_RAW }
}

// -- SyncBackend trait + DirectSync --

/// Abstraction over how two LoroDoc instances sync.
pub trait SyncBackend: Send + Sync {
    fn sync_pair(&self, doc_a: &LoroDoc, doc_b: &LoroDoc) -> anyhow::Result<()>;
}

/// Direct Loro sync using export/import — no network, deterministic, fast.
pub struct DirectSync;

impl SyncBackend for DirectSync {
    fn sync_pair(&self, a: &LoroDoc, b: &LoroDoc) -> anyhow::Result<()> {
        let b_vv = b.oplog_vv();
        let a_delta = a.export(ExportMode::updates(&b_vv))?;
        if !a_delta.is_empty() {
            b.import(&a_delta)?;
        }
        let a_vv = a.oplog_vv();
        let b_delta = b.export(ExportMode::updates(&a_vv))?;
        if !b_delta.is_empty() {
            a.import(&b_delta)?;
        }
        Ok(())
    }
}

// -- PeerState --

#[derive(Debug)]
pub struct PeerState<D: std::fmt::Debug = ()> {
    pub doc: LoroDoc,
    pub peer_id: u64,
    pub online: bool,
    pub data: D,
}

// -- GroupState --

pub struct GroupState<D: std::fmt::Debug = ()> {
    pub peers: Vec<PeerState<D>>,
    pub peer_counter: u64,
    pub last_transition_was_sync_all: bool,
    pub backend: Arc<dyn SyncBackend>,
}

impl<D: std::fmt::Debug> std::fmt::Debug for GroupState<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupState")
            .field("peers", &self.peers.len())
            .field("peer_counter", &self.peer_counter)
            .field(
                "last_transition_was_sync_all",
                &self.last_transition_was_sync_all,
            )
            .finish()
    }
}

impl<D: std::fmt::Debug + Clone> Clone for GroupState<D> {
    fn clone(&self) -> Self {
        Self {
            peers: self
                .peers
                .iter()
                .map(|p| PeerState {
                    doc: {
                        let forked = p.doc.fork();
                        forked.set_peer_id(p.peer_id).unwrap();
                        forked
                    },
                    peer_id: p.peer_id,
                    online: p.online,
                    data: p.data.clone(),
                })
                .collect(),
            peer_counter: self.peer_counter,
            last_transition_was_sync_all: self.last_transition_was_sync_all,
            backend: self.backend.clone(),
        }
    }
}

impl GroupState<()> {
    pub fn new(backend: Arc<dyn SyncBackend>) -> Self {
        let seed = init_doc(999);
        let snap = seed.export(ExportMode::Snapshot).unwrap();

        let peer1_doc = init_doc(1);
        peer1_doc.import(&snap).unwrap();
        let peer2_doc = init_doc(2);
        peer2_doc.import(&snap).unwrap();

        Self {
            peers: vec![
                PeerState {
                    doc: peer1_doc,
                    peer_id: 1,
                    online: true,
                    data: (),
                },
                PeerState {
                    doc: peer2_doc,
                    peer_id: 2,
                    online: true,
                    data: (),
                },
            ],
            peer_counter: 3,
            last_transition_was_sync_all: false,
            backend,
        }
    }
}

impl<D: std::fmt::Debug> GroupState<D> {
    pub fn online_indices(&self) -> Vec<usize> {
        self.peers
            .iter()
            .enumerate()
            .filter(|(_, p)| p.online)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn offline_indices(&self) -> Vec<usize> {
        self.peers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.online)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn alive_node_ids_for_peer(&self, peer_idx: usize) -> Vec<TreeID> {
        get_alive_nodes(&self.peers[peer_idx].doc)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    }
}

// -- Transitions --

#[derive(Clone, Debug)]
pub enum GroupTransition {
    Edit {
        peer_idx: usize,
        op: EditOp,
    },
    AddPeer,
    RemovePeer {
        peer_idx: usize,
    },
    GoOffline {
        peer_idx: usize,
    },
    GoOnline {
        peer_idx: usize,
    },
    SyncPair {
        from: usize,
        to: usize,
    },
    SyncAll,
    ReconnectWithNewPeerId {
        peer_idx: usize,
    },
    /// Shut down and re-create the system under test, preserving the Loro
    /// doc on disk. Used by the bridge PBT to exercise the startup reconcile
    /// path via `LoroSyncController`'s sidecar round-trip. A no-op on the
    /// pure reference state (it touches only the SUT).
    Restart,
    /// Simulate the background sync service: shut down the SUT, sync a peer
    /// into the primary while the SUT is dead, then restart. The merged
    /// Loro state must be reconciled via the startup path. A no-op on the
    /// pure reference state.
    OfflineMerge {
        peer_idx: usize,
    },
}

#[derive(Clone, Debug)]
pub enum EditOp {
    Create {
        parent_idx: usize,
        content: String,
    },
    Update {
        node_idx: usize,
        content: String,
    },
    Delete {
        node_idx: usize,
    },
    Move {
        node_idx: usize,
        new_parent_idx: usize,
    },
}

// -- LoroTree helpers --

pub fn init_doc(peer_id: u64) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer_id).unwrap();
    let tree = doc.get_tree(TREE_NAME);
    tree.enable_fractional_index(0);
    doc
}

pub fn get_alive_nodes(doc: &LoroDoc) -> Vec<(TreeID, Option<TreeID>, String)> {
    let tree = doc.get_tree(TREE_NAME);
    let mut result = Vec::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        let parent = match node.parent {
            loro::TreeParentId::Node(pid) => Some(pid),
            _ => None,
        };
        let content = read_text(&tree, node.id);
        result.push((node.id, parent, content));
    }
    result.sort_by_key(|(id, _, _)| (id.peer, id.counter));
    result
}

/// Extract the set of stable IDs from all alive nodes in a doc.
pub fn get_alive_stable_ids(doc: &LoroDoc) -> HashSet<String> {
    let tree = doc.get_tree(TREE_NAME);
    let mut ids = HashSet::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        if let Ok(meta) = tree.get_meta(node.id) {
            let sid = meta.get(STABLE_ID).and_then(|v| match v {
                ValueOrContainer::Value(val) => val.as_string().map(|s| s.to_string()),
                _ => None,
            });
            if let Some(sid) = sid {
                let is_new = ids.insert(sid.clone());
                assert!(
                    is_new,
                    "S3 FAILED: Duplicate STABLE_ID {:?} in single doc (peer {:?}:{:?})",
                    sid, node.id.peer, node.id.counter
                );
            }
        }
    }
    ids
}

pub fn read_text(tree: &LoroTree, node: TreeID) -> String {
    let meta = tree.get_meta(node).unwrap();
    let field = content_field_for(&meta);
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
        _ => String::new(),
    }
}

pub fn create_block(doc: &LoroDoc, parent: Option<TreeID>, content: &str) -> TreeID {
    let tree = doc.get_tree(TREE_NAME);
    let node = tree.create(parent).unwrap();
    let meta = tree.get_meta(node).unwrap();
    let text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();
    text.insert(0, content).unwrap();
    doc.commit();
    node
}

/// Create a block with a stable ID stored in metadata.
pub fn create_block_with_id(
    doc: &LoroDoc,
    parent: Option<TreeID>,
    content: &str,
    stable_id: &str,
) -> TreeID {
    let tree = doc.get_tree(TREE_NAME);
    let node = tree.create(parent).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id))
        .unwrap();
    let text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();
    text.insert(0, content).unwrap();
    doc.commit();
    node
}

pub fn update_block(doc: &LoroDoc, node: TreeID, new_content: &str) {
    let tree = doc.get_tree(TREE_NAME);
    let meta = tree.get_meta(node).unwrap();
    // Source blocks store their content in `source_code`, not `content_raw`.
    // Writing to the wrong field would leave the production reader returning
    // the original content even after `MergeFromPeer` imports the delta.
    let field = content_field_for(&meta);
    let text: LoroText = meta
        .get_or_create_container(field, LoroText::new())
        .unwrap();
    // Use `text.update()` (myers-diff, minimal RGA ops) to mirror the
    // production primary path (`update_text_field` in
    // `crates/holon/src/api/loro_backend.rs`). A naive
    // `delete(0, old_len) + insert(0, new)` produces different RGA
    // structure under shared-prefix scenarios — when peer's new content
    // happens to overlap the baseline, the wholesale rewrite re-inserts
    // tombstoned characters that race with primary's diff at position 0,
    // diverging from `loro_merge_text`'s reference prediction.
    text.update(new_content, Default::default()).unwrap();
    doc.commit();
}

pub fn delete_block(doc: &LoroDoc, node: TreeID) {
    let tree = doc.get_tree(TREE_NAME);
    tree.delete(node).unwrap();
    doc.commit();
}

pub fn move_block(
    doc: &LoroDoc,
    node: TreeID,
    new_parent: Option<TreeID>,
) -> Result<(), loro::LoroError> {
    let tree = doc.get_tree(TREE_NAME);
    tree.mov(node, new_parent)?;
    doc.commit();
    Ok(())
}

pub fn sync_docs_direct(a: &LoroDoc, b: &LoroDoc) {
    DirectSync.sync_pair(a, b).unwrap();
}

pub fn build_oracle<D: std::fmt::Debug>(peers: &[&PeerState<D>]) -> LoroDoc {
    let mut forks: Vec<LoroDoc> = peers.iter().map(|p| p.doc.fork()).collect();
    for _round in 0..3 {
        for i in 0..forks.len() {
            for j in (i + 1)..forks.len() {
                let (left, right) = forks.split_at_mut(j);
                sync_docs_direct(&left[i], &right[0]);
            }
        }
    }
    forks.remove(0)
}

// -- Transition generation, preconditions, application, invariants --

pub fn generate_transitions<D: std::fmt::Debug>(
    state: &GroupState<D>,
) -> BoxedStrategy<GroupTransition> {
    let online = state.online_indices();
    let offline = state.offline_indices();
    let peer_count = state.peers.len();

    let mut strategies: Vec<(u32, BoxedStrategy<GroupTransition>)> = Vec::new();

    if !online.is_empty() {
        let online_for_edit = online.clone();
        let sample_nodes = state.alive_node_ids_for_peer(online[0]);
        let node_count = sample_nodes.len();

        let edit_strat = if node_count == 0 {
            prop::sample::select(online_for_edit)
                .prop_flat_map(|peer_idx| {
                    "[a-z]{1,8}".prop_map(move |content| GroupTransition::Edit {
                        peer_idx,
                        op: EditOp::Create {
                            parent_idx: usize::MAX,
                            content,
                        },
                    })
                })
                .boxed()
        } else {
            let max_node = node_count.max(1);
            prop::sample::select(online_for_edit)
                .prop_flat_map(move |peer_idx| {
                    prop::strategy::Union::new_weighted(vec![
                        (
                            30,
                            (0..max_node, "[a-z]{1,8}")
                                .prop_map(move |(pi, c)| GroupTransition::Edit {
                                    peer_idx,
                                    op: EditOp::Create {
                                        parent_idx: pi,
                                        content: c,
                                    },
                                })
                                .boxed(),
                        ),
                        (
                            15,
                            (0..max_node, "[a-z]{1,8}")
                                .prop_map(move |(ni, c)| GroupTransition::Edit {
                                    peer_idx,
                                    op: EditOp::Update {
                                        node_idx: ni,
                                        content: c,
                                    },
                                })
                                .boxed(),
                        ),
                        (
                            10,
                            (0..max_node)
                                .prop_map(move |ni| GroupTransition::Edit {
                                    peer_idx,
                                    op: EditOp::Delete { node_idx: ni },
                                })
                                .boxed(),
                        ),
                        (
                            10,
                            (0..max_node, 0..max_node)
                                .prop_map(move |(ni, npi)| GroupTransition::Edit {
                                    peer_idx,
                                    op: EditOp::Move {
                                        node_idx: ni,
                                        new_parent_idx: npi,
                                    },
                                })
                                .boxed(),
                        ),
                    ])
                })
                .boxed()
        };
        strategies.push((65, edit_strat));
    }

    if online.len() >= 2 {
        let sync_pair = prop::sample::subsequence(online.clone(), 2)
            .prop_map(|pair| GroupTransition::SyncPair {
                from: pair[0],
                to: pair[1],
            })
            .boxed();
        strategies.push((10, sync_pair));
        strategies.push((10, Just(GroupTransition::SyncAll).boxed()));
    }

    if !online.is_empty() && peer_count > 1 {
        strategies.push((
            3,
            prop::sample::select(online.clone())
                .prop_map(|idx| GroupTransition::GoOffline { peer_idx: idx })
                .boxed(),
        ));
    }
    if !offline.is_empty() {
        strategies.push((
            3,
            prop::sample::select(offline)
                .prop_map(|idx| GroupTransition::GoOnline { peer_idx: idx })
                .boxed(),
        ));
    }

    if !online.is_empty() {
        strategies.push((2, Just(GroupTransition::AddPeer).boxed()));
    }
    if peer_count > 2 {
        strategies.push((
            1,
            prop::sample::select((0..peer_count).collect::<Vec<_>>())
                .prop_map(|idx| GroupTransition::RemovePeer { peer_idx: idx })
                .boxed(),
        ));
    }

    // Restart: SUT-only transition. A no-op on the pure reference state
    // but forces the bridge PBT's `LoroSyncController` through its sidecar
    // round-trip and startup reconcile path.
    strategies.push((1, Just(GroupTransition::Restart).boxed()));

    // OfflineMerge: simulates the background sync service — merge a peer's
    // changes into the primary doc while the controller is shut down, then
    // restart. SUT-only; reference state is unaffected.
    if online.len() >= 2 {
        let online_for_offline = online.clone();
        strategies.push((
            2,
            prop::sample::select(online_for_offline)
                .prop_map(|idx| GroupTransition::OfflineMerge { peer_idx: idx })
                .boxed(),
        ));
    }

    if !online.is_empty() {
        strategies.push((
            1,
            prop::sample::select(online)
                .prop_map(|idx| GroupTransition::ReconnectWithNewPeerId { peer_idx: idx })
                .boxed(),
        ));
    }

    assert!(!strategies.is_empty());
    prop::strategy::Union::new_weighted(strategies).boxed()
}

pub fn check_preconditions<D: std::fmt::Debug>(
    state: &GroupState<D>,
    transition: &GroupTransition,
) -> bool {
    match transition {
        GroupTransition::Edit { peer_idx, .. } => {
            *peer_idx < state.peers.len() && state.peers[*peer_idx].online
        }
        GroupTransition::SyncPair { from, to } => {
            *from < state.peers.len()
                && *to < state.peers.len()
                && from != to
                && state.peers[*from].online
                && state.peers[*to].online
        }
        GroupTransition::SyncAll => state.online_indices().len() >= 2,
        GroupTransition::GoOffline { peer_idx } => {
            *peer_idx < state.peers.len()
                && state.peers[*peer_idx].online
                && state.online_indices().len() > 1
        }
        GroupTransition::GoOnline { peer_idx } => {
            *peer_idx < state.peers.len() && !state.peers[*peer_idx].online
        }
        GroupTransition::AddPeer => !state.online_indices().is_empty(),
        GroupTransition::RemovePeer { peer_idx } => {
            *peer_idx < state.peers.len() && state.peers.len() > 2
        }
        GroupTransition::ReconnectWithNewPeerId { peer_idx } => *peer_idx < state.peers.len(),
        GroupTransition::Restart => true,
        GroupTransition::OfflineMerge { peer_idx } => {
            *peer_idx < state.peers.len() && state.peers[*peer_idx].online
        }
    }
}

pub fn apply_transition(mut state: GroupState<()>, transition: &GroupTransition) -> GroupState<()> {
    state.last_transition_was_sync_all = false;
    let backend = state.backend.clone();

    match transition {
        GroupTransition::Edit { peer_idx, op } => {
            let nodes = state.alive_node_ids_for_peer(*peer_idx);
            let peer = &state.peers[*peer_idx];

            match op {
                EditOp::Create {
                    parent_idx,
                    content,
                } => {
                    let parent = if *parent_idx == usize::MAX || nodes.is_empty() {
                        None
                    } else {
                        Some(nodes[*parent_idx % nodes.len()])
                    };
                    // Use `create_block_with_id` so every created node
                    // carries a stable UUID — `holon_loro`
                    // requires STABLE_ID metadata on every tree node.
                    let stable_id = uuid::Uuid::new_v4().to_string();
                    create_block_with_id(&peer.doc, parent, content, &stable_id);
                }
                EditOp::Update { node_idx, content } => {
                    if !nodes.is_empty() {
                        update_block(&peer.doc, nodes[*node_idx % nodes.len()], content);
                    }
                }
                EditOp::Delete { node_idx } => {
                    if !nodes.is_empty() {
                        delete_block(&peer.doc, nodes[*node_idx % nodes.len()]);
                    }
                }
                EditOp::Move {
                    node_idx,
                    new_parent_idx,
                } => {
                    if !nodes.is_empty() {
                        let node = nodes[*node_idx % nodes.len()];
                        let new_parent = if nodes.len() == 1 {
                            None
                        } else {
                            Some(nodes[*new_parent_idx % nodes.len()])
                        };
                        let _ = move_block(&peer.doc, node, new_parent);
                    }
                }
            }
        }

        GroupTransition::SyncPair { from, to } => {
            backend
                .sync_pair(&state.peers[*from].doc, &state.peers[*to].doc)
                .unwrap();
        }

        GroupTransition::SyncAll => {
            let online = state.online_indices();
            for _round in 0..3 {
                for i in 0..online.len() {
                    for j in (i + 1)..online.len() {
                        backend
                            .sync_pair(&state.peers[online[i]].doc, &state.peers[online[j]].doc)
                            .unwrap();
                    }
                }
            }
            state.last_transition_was_sync_all = true;
        }

        GroupTransition::GoOffline { peer_idx } => {
            state.peers[*peer_idx].online = false;
        }
        GroupTransition::GoOnline { peer_idx } => {
            state.peers[*peer_idx].online = true;
        }

        GroupTransition::AddPeer => {
            let peer_id = state.peer_counter;
            state.peer_counter += 1;
            let doc = init_doc(peer_id);
            let online = state.online_indices();
            let snap = state.peers[online[0]]
                .doc
                .export(ExportMode::Snapshot)
                .unwrap();
            doc.import(&snap).unwrap();
            state.peers.push(PeerState {
                doc,
                peer_id,
                online: true,
                data: (),
            });
        }

        GroupTransition::RemovePeer { peer_idx } => {
            state.peers.remove(*peer_idx);
        }

        GroupTransition::ReconnectWithNewPeerId { peer_idx } => {
            let new_id = state.peer_counter;
            state.peer_counter += 1;
            state.peers[*peer_idx].doc.set_peer_id(new_id).unwrap();
            state.peers[*peer_idx].peer_id = new_id;
        }

        // Restart and OfflineMerge are SUT-only transitions. The reference
        // state is unaffected — the SUT implements whatever effect they
        // have on the system under test (e.g. shutting down a
        // `LoroSyncController` and reading its sidecar back on restart).
        GroupTransition::Restart => {}
        GroupTransition::OfflineMerge { .. } => {}
    }

    state
}

pub fn check_invariants<D: std::fmt::Debug>(ref_state: &GroupState<D>) {
    // S1, S2: Per-peer structural invariants
    for (idx, peer) in ref_state.peers.iter().enumerate() {
        let nodes = get_alive_nodes(&peer.doc);
        let alive_ids: HashSet<TreeID> = nodes.iter().map(|(id, _, _)| *id).collect();

        for (id, parent, _) in &nodes {
            if let Some(pid) = parent {
                assert!(
                    alive_ids.contains(pid),
                    "S1 FAILED: Peer {} node {:?} has dead parent {:?}",
                    idx,
                    id,
                    pid
                );
            }
        }
        assert_eq!(
            alive_ids.len(),
            nodes.len(),
            "S2 FAILED: Peer {} has duplicate TreeIDs",
            idx
        );
    }

    // S3: Per-peer stable ID uniqueness (no two alive nodes share a STABLE_ID)
    for peer in &ref_state.peers {
        get_alive_stable_ids(&peer.doc);
    }

    // C1-C3: Convergence invariants (trial sync on clones, always uses DirectSync)
    let online = ref_state.online_indices();
    if online.len() >= 2 {
        let mut trial_docs: Vec<LoroDoc> = online
            .iter()
            .map(|&idx| ref_state.peers[idx].doc.fork())
            .collect();

        for _round in 0..3 {
            for i in 0..trial_docs.len() {
                for j in (i + 1)..trial_docs.len() {
                    let (left, right) = trial_docs.split_at_mut(j);
                    sync_docs_direct(&left[i], &right[0]);
                }
            }
        }

        let online_peers: Vec<&PeerState<D>> =
            online.iter().map(|&idx| &ref_state.peers[idx]).collect();
        let oracle = build_oracle(&online_peers);
        let oracle_nodes = get_alive_nodes(&oracle);

        for (trial_idx, &peer_idx) in online.iter().enumerate() {
            let trial_nodes = get_alive_nodes(&trial_docs[trial_idx]);
            assert_eq!(
                trial_nodes,
                oracle_nodes,
                "C1 FAILED: Peer {} diverges from oracle after trial sync.\nPeer {}: {} nodes \
                 {:?}\nOracle: {} nodes {:?}",
                peer_idx,
                peer_idx,
                trial_nodes.len(),
                trial_nodes
                    .iter()
                    .map(|(_, _, c)| c.as_str())
                    .collect::<Vec<_>>(),
                oracle_nodes.len(),
                oracle_nodes
                    .iter()
                    .map(|(_, _, c)| c.as_str())
                    .collect::<Vec<_>>(),
            );
        }

        for i in 0..trial_docs.len() {
            for j in (i + 1)..trial_docs.len() {
                let before = get_alive_nodes(&trial_docs[i]);
                let (left, right) = trial_docs.split_at_mut(j);
                sync_docs_direct(&left[i], &right[0]);
                let after = get_alive_nodes(&left[i]);
                assert_eq!(
                    before, after,
                    "C2 FAILED: Extra sync round changed state for peer {}",
                    online[i]
                );
            }
        }

        if ref_state.last_transition_was_sync_all {
            for &idx in &online {
                let actual_nodes = get_alive_nodes(&ref_state.peers[idx].doc);
                assert_eq!(
                    actual_nodes,
                    oracle_nodes,
                    "C3 FAILED: Peer {} didn't converge after SyncAll.\nActual: {} nodes \
                     {:?}\nOracle: {} nodes {:?}",
                    idx,
                    actual_nodes.len(),
                    actual_nodes
                        .iter()
                        .map(|(_, _, c)| c.as_str())
                        .collect::<Vec<_>>(),
                    oracle_nodes.len(),
                    oracle_nodes
                        .iter()
                        .map(|(_, _, c)| c.as_str())
                        .collect::<Vec<_>>(),
                );
            }

            // I3: After SyncAll, all online peers have identical stable ID sets
            let reference_ids = get_alive_stable_ids(&ref_state.peers[online[0]].doc);
            for &idx in &online[1..] {
                let peer_ids = get_alive_stable_ids(&ref_state.peers[idx].doc);
                assert_eq!(
                    reference_ids,
                    peer_ids,
                    "I3 FAILED: Peer {} has different stable IDs than peer {} after \
                     SyncAll.\nOnly in peer {}: {:?}\nOnly in peer {}: {:?}",
                    idx,
                    online[0],
                    online[0],
                    reference_ids.difference(&peer_ids).collect::<Vec<_>>(),
                    idx,
                    peer_ids.difference(&reference_ids).collect::<Vec<_>>(),
                );
            }
        }
    }

    // V1: VV size bounded
    for (idx, peer) in ref_state.peers.iter().enumerate() {
        let vv = peer.doc.oplog_vv();
        let vv_size = vv.encode().len();
        let max_reasonable = 16 * (ref_state.peer_counter as usize + 5);
        assert!(
            vv_size <= max_reasonable,
            "V1 FAILED: Peer {} VV is {} bytes (peer_counter={}).\nPossible unbounded growth from \
             changing peer_ids.",
            idx,
            vv_size,
            ref_state.peer_counter
        );
    }
}

// ─── Lamport clock helpers (E-solid oracle clock-sync seam) ──────────────────

/// A doc's Lamport height: 1 + the max lamport of any applied op, computed
/// from public API only (frontiers + `ChangeMeta`). This scalar is the ONLY
/// value the E-solid shadow-mesh oracle reads from the SUT (clock sync at
/// fork/sync boundaries); see `clock_parity_spike` for the parity proof and
/// the negative control showing the padding is load-bearing.
pub fn lamport_height(doc: &LoroDoc) -> u32 {
    crate::loro_backend::doc_lamport_height(doc)
}

/// Advance `doc`'s Lamport clock to exactly `target` via 1-atom ops in a
/// scratch container. Query-driven — no assumption that N ops advance the
/// clock by N; each step re-reads the height, and overshoot panics loudly
/// (it would falsify the whole clock-padding approach).
pub fn pad_to_height(doc: &LoroDoc, target: u32) {
    let pad = doc.get_text("clock_pad");
    loop {
        let h = lamport_height(doc);
        assert!(h <= target, "shadow clock overshot: {h} > {target}");
        if h == target {
            return;
        }
        pad.insert(0, "x").unwrap();
        doc.commit();
    }
}

// ─── E-solid de-risk spike: shadow-mesh clock-padding parity ─────────────────
//
// Question under test: can a SHADOW peer universe — a fresh primary doc whose
// Lamport clock is padded to the real universe's observed scalar heights at
// each fork/sync boundary, with the same logical peer ops applied through the
// SAME helpers — reproduce the real universe's EXACT tied-sibling order and
// concurrent-text interleaving, despite the real primary carrying op history
// (boot seeding, engine writes) the shadow never replays?
//
// If yes, a pure-ish oracle (only scalar clock reads cross from the SUT) can
// PREDICT the CRDT tie-breaks exactly, replacing check-time SUT adoption.
// These tests double as parity teeth: a loro upgrade that changes op-atom
// encoding or tie-break semantics fails HERE, deterministically, naming the
// model — instead of as a misattributed keystone PBT red.
#[cfg(test)]
mod clock_parity_spike {
    use super::*;

    fn stable_id_of(tree: &LoroTree, node: TreeID) -> Option<String> {
        let meta = tree.get_meta(node).ok()?;
        match meta.get(STABLE_ID) {
            Some(ValueOrContainer::Value(v)) => v.as_string().map(|s| s.to_string()),
            _ => None,
        }
    }

    fn node_by_stable_id(doc: &LoroDoc, stable_id: &str) -> TreeID {
        let tree = doc.get_tree(TREE_NAME);
        tree.get_nodes(false)
            .into_iter()
            .filter(|n| {
                !matches!(
                    n.parent,
                    loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                )
            })
            .find(|n| stable_id_of(&tree, n.id).as_deref() == Some(stable_id))
            .map(|n| n.id)
            .unwrap_or_else(|| panic!("node {stable_id} not found"))
    }

    /// Children stable-ids of `parent_sid` in the tree's true order.
    fn children_order(doc: &LoroDoc, parent_sid: &str) -> Vec<String> {
        let tree = doc.get_tree(TREE_NAME);
        let parent = node_by_stable_id(doc, parent_sid);
        tree.children(parent)
            .unwrap_or_default()
            .into_iter()
            .map(|c| stable_id_of(&tree, c).expect("child has stable id"))
            .collect()
    }

    fn content_of(doc: &LoroDoc, sid: &str) -> String {
        let tree = doc.get_tree(TREE_NAME);
        read_text(&tree, node_by_stable_id(doc, sid))
    }

    /// One peer universe: a primary doc + forked peers, seeded with the same
    /// parent/c1/c2 working tree. The REAL universe additionally carries
    /// arbitrary non-shared history (junk ops simulating boot seeding /
    /// engine writes); the SHADOW universe carries none and is clock-padded.
    struct Universe {
        primary: LoroDoc,
        peers: Vec<LoroDoc>,
    }

    impl Universe {
        fn new() -> Self {
            let primary = init_doc(1);
            let parent = create_block_with_id(&primary, None, "parent", "parent");
            create_block_with_id(&primary, Some(parent), "c1", "c1");
            create_block_with_id(&primary, Some(parent), "c2", "c2");
            Self {
                primary,
                peers: Vec::new(),
            }
        }

        /// Simulated non-shared primary history (boot seeding, engine writes):
        /// ops in a scratch container the peers/oracle never look at.
        fn junk(&self, n: usize) {
            let t = self.primary.get_text("boot_junk");
            for _ in 0..n {
                t.insert(0, "j").unwrap();
                self.primary.commit();
            }
        }

        fn add_peer(&mut self, peer_id: u64) {
            let snapshot = self.primary.export(ExportMode::Snapshot).unwrap();
            let doc = init_doc(peer_id);
            doc.import(&snapshot).unwrap();
            self.peers.push(doc);
        }

        fn peer_create(&self, idx: usize, parent_sid: &str, content: &str, sid: &str) {
            let parent = node_by_stable_id(&self.peers[idx], parent_sid);
            create_block_with_id(&self.peers[idx], Some(parent), content, sid);
        }

        fn peer_update(&self, idx: usize, sid: &str, content: &str) {
            let node = node_by_stable_id(&self.peers[idx], sid);
            update_block(&self.peers[idx], node, content);
        }

        fn sync(&self, idx: usize) {
            sync_docs_direct(&self.primary, &self.peers[idx]);
        }

        fn height(&self) -> u32 {
            lamport_height(&self.primary)
        }
    }

    /// Drive the same logical script through a REAL universe (with junk
    /// history) and a SHADOW universe (clock-padded from the real one's
    /// scalar heights), then assert EXACT parity of sibling order and text.
    ///
    /// `script` receives (universe, is_shadow, &mut clock) where `clock`
    /// yields the real universe's recorded heights to the shadow run.
    fn assert_parity(
        script: impl Fn(
            &mut Universe,
            &mut dyn FnMut(&Universe) -> u32,
            &mut dyn FnMut(&Universe, usize),
        ),
        parents_to_check: &[&str],
        texts_to_check: &[&str],
    ) {
        // Real run: junk (non-shared history) is applied; heights are read
        // live off the real primary at each boundary and recorded.
        let mut recorded: Vec<u32> = Vec::new();
        let mut real = Universe::new();
        {
            let mut clock = |u: &Universe| {
                let h = u.height();
                recorded.push(h);
                h
            };
            let mut junk = |u: &Universe, n: usize| u.junk(n);
            script(&mut real, &mut clock, &mut junk);
        }

        // Shadow run: junk is a NO-OP (the shadow never sees that history —
        // exactly the oracle's position); boundary heights are replayed from
        // the recording and the shadow primary is PADDED to each.
        let mut replay = recorded.clone().into_iter();
        let mut shadow = Universe::new();
        {
            let mut clock = |u: &Universe| {
                let target = replay.next().expect("shadow consumed more clock reads");
                pad_to_height(&u.primary, target);
                target
            };
            let mut junk = |_: &Universe, _: usize| {};
            script(&mut shadow, &mut clock, &mut junk);
        }

        for parent in parents_to_check {
            assert_eq!(
                children_order(&real.primary, parent),
                children_order(&shadow.primary, parent),
                "sibling order diverged under {parent}"
            );
        }
        for sid in texts_to_check {
            assert_eq!(
                content_of(&real.primary, sid),
                content_of(&shadow.primary, sid),
                "text content diverged for {sid}"
            );
        }
    }

    /// S1 — equal clocks, reversed creation: higher peer id creates FIRST.
    /// Real tie order must be (lamport, peer id); the shadow must reproduce it.
    #[test]
    fn s1_equal_clock_reversed_creation_order() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 37);
                clock(u);
                u.add_peer(100);
                clock(u);
                u.add_peer(101);
                u.peer_create(1, "parent", "created-first", "peer-high");
                u.peer_create(0, "parent", "created-second", "peer-low");
                clock(u);
                u.sync(1);
                clock(u);
                u.sync(0);
            },
            &["parent"],
            &[],
        );
    }

    /// S2 — lamport bump: the LOWER peer id raises its clock with unrelated
    /// edits before creating; lamport must dominate peer id, shadow included.
    #[test]
    fn s2_lamport_bumped_create() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 12);
                clock(u);
                u.add_peer(100);
                clock(u);
                u.add_peer(101);
                for _ in 0..5 {
                    u.peer_update(0, "c2", "bump-bump-bump");
                }
                u.peer_create(0, "parent", "low-id-high-lamport", "peer-low");
                u.peer_create(1, "parent", "high-id-low-lamport", "peer-high");
                clock(u);
                u.sync(1);
                clock(u);
                u.sync(0);
            },
            &["parent"],
            &["c2"],
        );
    }

    /// S3 — staggered forks: the primary advances (junk) BETWEEN AddPeers, so
    /// the two peers fork at different base heights. This is exactly the case
    /// an unpadded shadow gets wrong.
    #[test]
    fn s3_staggered_fork_heights() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 8);
                clock(u);
                u.add_peer(100);
                junk(u, 9); // primary advances between forks
                clock(u);
                u.add_peer(101);
                u.peer_create(0, "parent", "from-early-fork", "peer-early");
                u.peer_create(1, "parent", "from-late-fork", "peer-late");
                clock(u);
                u.sync(0);
                clock(u);
                u.sync(1);
            },
            &["parent"],
            &[],
        );
    }

    /// S4 — concurrent text updates: the merged INTERLEAVING (not just the
    /// multiset) must match exactly.
    #[test]
    fn s4_concurrent_text_interleaving() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 23);
                clock(u);
                u.add_peer(100);
                clock(u);
                u.add_peer(101);
                u.peer_update(0, "c1", "daaa");
                u.peer_update(1, "c1", "daab");
                clock(u);
                u.sync(1);
                clock(u);
                u.sync(0);
                clock(u);
                u.sync(1);
            },
            &["parent"],
            &["c1"],
        );
    }

    /// S5 — causal (non-concurrent) creates: sanity that padding does not
    /// disturb the already-modelable case.
    #[test]
    fn s5_causal_creates_stay_ordered() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 5);
                clock(u);
                u.add_peer(100);
                u.peer_create(0, "parent", "first", "peer-a");
                clock(u);
                u.sync(0);
                clock(u);
                u.add_peer(101); // forks AFTER peer-a landed
                u.peer_create(1, "parent", "second", "peer-b");
                clock(u);
                u.sync(1);
            },
            &["parent"],
            &[],
        );
    }

    /// S8 — base-shape independence: the REAL universe's seed is built with a
    /// different primary peer id AND different op granularity (content written
    /// char-by-char in separate commits) than the shadow's one-shot seed. The
    /// production global doc's base ops (org-scan boot) likewise differ from
    /// anything the oracle replays — concurrent-edit ordering must not depend
    /// on the BASE ops' ids, only on the base string + the peers' own op ids.
    #[test]
    fn s8_base_op_shape_independence() {
        fn seed_weird(primary: &LoroDoc) -> Universe {
            // parent/c1/c2 with the same STRINGS but different op shapes:
            // empty create, then per-char text inserts in separate commits.
            let tree = primary.get_tree(TREE_NAME);
            for (sid, content, parent_sid) in [
                ("parent", "parent", None),
                ("c1", "c1", Some("parent")),
                ("c2", "c2", Some("parent")),
            ] {
                let parent = parent_sid.map(|p| node_by_stable_id(primary, p));
                let node = tree.create(parent).unwrap();
                let meta = tree.get_meta(node).unwrap();
                meta.insert(STABLE_ID, loro::LoroValue::from(sid)).unwrap();
                let text: LoroText = meta.insert_container(CONTENT_RAW, LoroText::new()).unwrap();
                primary.commit();
                for (i, ch) in content.chars().enumerate() {
                    text.insert(i, &ch.to_string()).unwrap();
                    primary.commit();
                }
            }
            Universe {
                primary: primary.clone(),
                peers: Vec::new(),
            }
        }

        let script = |u: &mut Universe,
                      clock: &mut dyn FnMut(&Universe) -> u32,
                      _junk: &mut dyn FnMut(&Universe, usize)| {
            clock(u);
            u.add_peer(100);
            clock(u);
            u.add_peer(101);
            u.peer_update(1, "c1", "daab");
            u.peer_update(0, "c1", "daaa");
            u.peer_create(1, "parent", "b-block", "peer-b");
            u.peer_create(0, "parent", "a-block", "peer-a");
            clock(u);
            u.sync(1);
            clock(u);
            u.sync(0);
            clock(u);
            u.sync(1);
        };

        // Real: weird seed, different primary peer id (7777).
        let mut recorded: Vec<u32> = Vec::new();
        let mut real = seed_weird(&init_doc(7777));
        {
            let mut clock = |u: &Universe| {
                let h = u.height();
                recorded.push(h);
                h
            };
            let mut junk = |u: &Universe, n: usize| u.junk(n);
            script(&mut real, &mut clock, &mut junk);
        }

        // Shadow: standard one-shot seed, primary peer id 1, clock-padded.
        let mut replay = recorded.into_iter();
        let mut shadow = Universe::new();
        {
            let mut clock = |u: &Universe| {
                let target = replay.next().expect("clock reads exhausted");
                pad_to_height(&u.primary, target);
                target
            };
            let mut junk = |_: &Universe, _: usize| {};
            script(&mut shadow, &mut clock, &mut junk);
        }

        assert_eq!(
            children_order(&real.primary, "parent"),
            children_order(&shadow.primary, "parent"),
            "sibling order must not depend on base op shapes"
        );
        assert_eq!(
            content_of(&real.primary, "c1"),
            content_of(&shadow.primary, "c1"),
            "merged interleaving must not depend on base op shapes"
        );
    }

    /// S7 — NEGATIVE CONTROL: without padding, the shadow universe DOES
    /// diverge on the reversed-stagger shape (higher peer id forks earlier at
    /// a lower height; real order = lamport order, unpadded shadow collapses
    /// to equal lamports → peer-id order). Proves the padding is load-bearing
    /// and the parity assertions above have teeth. If loro ever changes so
    /// this stops diverging, the whole clock-sync mechanism needs re-review.
    #[test]
    fn s7_unpadded_shadow_diverges_negative_control() {
        let script = |u: &mut Universe,
                      clock: &mut dyn FnMut(&Universe) -> u32,
                      junk: &mut dyn FnMut(&Universe, usize)| {
            clock(u);
            u.add_peer(101); // HIGHER id forks EARLY (low height)
            junk(u, 9); //     primary advances…
            clock(u);
            u.add_peer(100); // …LOWER id forks LATE (high height)
            u.peer_create(0, "parent", "from-early-high-id", "peer-early-101");
            u.peer_create(1, "parent", "from-late-low-id", "peer-late-100");
            clock(u);
            u.sync(0);
            clock(u);
            u.sync(1);
        };
        // Peer index note: peers[0] = id 101 (added first), peers[1] = id 100.

        // Padded shadow must match (same machinery as assert_parity).
        assert_parity(|u, clock, junk| script(u, clock, junk), &["parent"], &[]);

        // UNPADDED shadow must DIVERGE — otherwise the parity tests are vacuous.
        let mut real = Universe::new();
        {
            let mut clock = |u: &Universe| u.height();
            let mut junk = |u: &Universe, n: usize| u.junk(n);
            script(&mut real, &mut clock, &mut junk);
        }
        let mut naive = Universe::new();
        {
            let mut clock = |_: &Universe| 0; // no read, no padding
            let mut junk = |_: &Universe, _: usize| {}; // no junk either
            script(&mut naive, &mut clock, &mut junk);
        }
        assert_ne!(
            children_order(&real.primary, "parent"),
            children_order(&naive.primary, "parent"),
            "unpadded shadow agreed with the real universe — the negative control lost its teeth; \
             re-review the clock-sync mechanism"
        );
    }

    /// S6 — the kitchen sink: staggered forks + lamport bumps + reversed
    /// creation + concurrent text on two blocks + a third peer.
    #[test]
    fn s6_combined_scenario() {
        assert_parity(
            |u, clock, junk| {
                junk(u, 31);
                clock(u);
                u.add_peer(100);
                junk(u, 4);
                clock(u);
                u.add_peer(101);
                u.peer_update(1, "c1", "from-b");
                u.peer_update(0, "c1", "from-a");
                for _ in 0..3 {
                    u.peer_update(1, "c2", "bump");
                }
                u.peer_create(1, "parent", "b-block", "peer-b");
                u.peer_create(0, "parent", "a-block", "peer-a");
                junk(u, 6);
                clock(u);
                u.add_peer(102);
                u.peer_create(2, "parent", "c-block", "peer-c");
                clock(u);
                u.sync(2);
                clock(u);
                u.sync(0);
                clock(u);
                u.sync(1);
                clock(u);
                u.sync(2);
            },
            &["parent"],
            &["c1", "c2"],
        );
    }

    /// Deep-clone every doc in a universe via `fork()` +
    /// `set_peer_id(original)`. This is the ShadowDoc `Clone` strategy:
    /// fork mints a NEW random peer id, so continuing ops under the fork
    /// unrestored would change tie-breaks; restoring the original id must
    /// let op counters continue seamlessly.
    fn deep_clone(u: &Universe) -> Universe {
        let clone_doc = |doc: &LoroDoc| {
            let pid = doc.peer_id();
            let forked = doc.fork();
            forked
                .set_peer_id(pid)
                .expect("restore original peer id on fork");
            forked
        };
        Universe {
            primary: clone_doc(&u.primary),
            peers: u.peers.iter().map(clone_doc).collect(),
        }
    }

    /// S9 — ShadowDoc Clone de-risk: fork+set_peer_id mid-script (twice, one
    /// clone-of-clone, matching proptest's per-step + per-case ref cloning)
    /// must produce EXACTLY the outcomes of a never-cloned run — including a
    /// peer-id tie-break minted AFTER the clones — and the clone must be a
    /// deep copy (post-clone ops on the original must not leak in).
    #[test]
    fn s9_fork_set_peer_id_clone_preserves_predictions() {
        let half1 = |u: &mut Universe| {
            u.junk(9);
            u.add_peer(100);
            u.junk(4);
            u.add_peer(101);
            u.peer_create(1, "parent", "pre-clone-high", "pre-high");
            u.peer_update(0, "c1", "half1-edit");
        };
        // Equal-lamport concurrent creates AFTER the clone: order depends on
        // the ops carrying peer ids 100 vs 101 — a fork-minted random peer id
        // would scramble this.
        let half2a = |u: &mut Universe| {
            u.peer_create(1, "parent", "post-clone-high", "post-high");
            u.peer_create(0, "parent", "post-clone-low", "post-low");
        };
        let half2b = |u: &mut Universe| {
            u.peer_update(0, "c2", "from-low");
            u.peer_update(1, "c2", "from-high");
            u.sync(1);
            u.sync(0);
            u.sync(1);
        };

        let mut baseline = Universe::new();
        half1(&mut baseline);
        half2a(&mut baseline);
        half2b(&mut baseline);

        let mut original = Universe::new();
        half1(&mut original);
        let cloned = deep_clone(&original);
        let h_cloned = lamport_height(&cloned.primary);
        // Deep-copy independence: ops on the ORIGINAL after cloning must not
        // reach the clone (an Arc-shared alias would corrupt proptest replays).
        original.junk(5);
        original.peer_create(1, "parent", "orig-only", "orig-only");
        assert_eq!(
            lamport_height(&cloned.primary),
            h_cloned,
            "post-clone ops on the original leaked into the clone — fork is not a deep copy"
        );

        let mut cloned = cloned;
        half2a(&mut cloned);
        let mut clone_of_clone = deep_clone(&cloned);
        half2b(&mut clone_of_clone);

        assert_eq!(
            children_order(&baseline.primary, "parent"),
            children_order(&clone_of_clone.primary, "parent"),
            "sibling order diverged after fork+set_peer_id cloning"
        );
        for sid in ["c1", "c2"] {
            assert_eq!(
                content_of(&baseline.primary, sid),
                content_of(&clone_of_clone.primary, sid),
                "text diverged for {sid} after fork+set_peer_id cloning"
            );
        }
        assert!(
            !children_order(&clone_of_clone.primary, "parent")
                .iter()
                .any(|s| s == "orig-only"),
            "original's post-clone create leaked into the clone lineage"
        );
    }
}
