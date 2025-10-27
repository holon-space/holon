//! Shared multi-peer sync infrastructure for property-based testing.
//!
//! Provides `PeerState`, `GroupState`, `GroupTransition`, and helpers for
//! generating, applying, and checking multi-peer Loro sync scenarios.
//! Used by both `sync_pbt` (unit-level) and `general_e2e_pbt` (integration).

use loro::{Container, ExportMode, LoroDoc, LoroText, LoroTree, TreeID, ValueOrContainer};
use proptest::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;

pub use crate::api::loro_backend::{CONTENT_RAW, CONTENT_TYPE, SOURCE_CODE, STABLE_ID, TREE_NAME};

/// Return the metadata field that stores a node's primary text content,
/// based on its `content_type` (`source` blocks live in `source_code`,
/// everything else lives in `content_raw`).
fn content_field_for(meta: &loro::LoroMap) -> &'static str {
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
    let old_len = text.len_unicode();
    if old_len > 0 {
        text.delete(0, old_len).unwrap();
    }
    text.insert(0, new_content).unwrap();
    doc.commit();
}

pub fn delete_block(doc: &LoroDoc, node: TreeID) {
    let tree = doc.get_tree(TREE_NAME);
    tree.delete(node).unwrap();
    doc.commit();
}

pub fn move_block(doc: &LoroDoc, node: TreeID, new_parent: Option<TreeID>) -> Result<(), ()> {
    let tree = doc.get_tree(TREE_NAME);
    tree.mov(node, new_parent).map_err(|_| ())?;
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
                    // carries a stable UUID — `holon::api::loro_backend`
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
                "C1 FAILED: Peer {} diverges from oracle after trial sync.\n\
                 Peer {}: {} nodes {:?}\n\
                 Oracle: {} nodes {:?}",
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
                    "C3 FAILED: Peer {} didn't converge after SyncAll.\n\
                     Actual: {} nodes {:?}\n\
                     Oracle: {} nodes {:?}",
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
                    "I3 FAILED: Peer {} has different stable IDs than peer {} after SyncAll.\n\
                     Only in peer {}: {:?}\n\
                     Only in peer {}: {:?}",
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
            "V1 FAILED: Peer {} VV is {} bytes (peer_counter={}).\n\
             Possible unbounded growth from changing peer_ids.",
            idx,
            vv_size,
            ref_state.peer_counter
        );
    }
}
