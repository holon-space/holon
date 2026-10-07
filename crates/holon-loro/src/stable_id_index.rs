//! `stable id -> TreeID` for every live, settled node of one doc's block tree.
//!
//! Complete: a miss means no live node carries the id. The index lives in the
//! doc's registry entry beside its boundary lock, so every wrapper of one
//! `LoroDoc` shares it and a new doc starts with an empty one.
//!
//! It hears committed changes through a [`TreeWatch`]. An open write batch's
//! own ops are not committed yet; [`write_stable_id`] notes the node it writes,
//! so a lookup inside the batch sees the batch's new ids, and [`revert_to`]
//! rebuilds the index. Any other uncommitted op that makes a node carry an id
//! (a raw move that revives a deleted node) is found by a tree scan when a
//! lookup misses while ops are pending; until its commit, a lookup that hits
//! may still name a larger duplicate than the revived node.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use loro::LoroDoc;
use loro::TreeID;

use crate::loro_backend::STABLE_ID;
use crate::loro_backend::TREE_NAME;
use crate::settled_read::LiveNode;
use crate::settled_read::classify;
use crate::tree_watch::Drained;
use crate::tree_watch::TreeWatch;

pub(crate) type StableIds = Arc<Mutex<StableIdIndex>>;

/// How much tree the index has read since the doc was created.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StableIdIndexStats {
    /// Walks of the whole tree: the first lookup, and each burst past the
    /// touched-node limit (a large import).
    pub full_builds: u64,
    /// Nodes read by full builds, drains of touched subtrees and open-batch
    /// scans.
    pub nodes_visited: u64,
    /// Tree scans of a lookup that missed while the doc had uncommitted ops.
    pub open_batch_scans: u64,
}

pub(crate) struct StableIdIndex {
    built: bool,
    /// Every live carrier of an id, ascending; the first is canonical.
    live: HashMap<String, Vec<TreeID>>,
    by_node: HashMap<TreeID, String>,
    watch: TreeWatch,
    stats: StableIdIndexStats,
    /// The doc version the index was last checked against; `None` after a
    /// build.
    #[cfg(debug_assertions)]
    checked: Option<loro::Frontiers>,
}

impl Default for StableIdIndex {
    fn default() -> Self {
        Self {
            built: false,
            live: HashMap::new(),
            by_node: HashMap::new(),
            watch: TreeWatch::new(&[], &[STABLE_ID]),
            stats: StableIdIndexStats::default(),
            #[cfg(debug_assertions)]
            checked: None,
        }
    }
}

/// Write `stable_id` into `node`'s meta. The only production writer of
/// `STABLE_ID`: it also tells the doc's index, which hears the write's commit
/// only later.
pub fn write_stable_id(doc: &LoroDoc, node: TreeID, stable_id: &str) -> anyhow::Result<()> {
    doc.get_tree(TREE_NAME)
        .get_meta(node)?
        .insert(STABLE_ID, stable_id)?;
    // A doc no lock was ever made for has no index yet; its first lookup
    // builds from the tree.
    if let Some(index) = crate::doc_lock::stable_ids_of(doc) {
        index.lock().unwrap().watch.note(node);
    }
    Ok(())
}

/// Carry `doc` back to `version` inside an open write batch. The revert can
/// re-create any node, so the doc's index rebuilds at its next lookup.
pub(crate) fn revert_to(doc: &LoroDoc, version: &loro::Frontiers) -> loro::LoroResult<()> {
    doc.revert_to(version)?;
    if let Some(index) = crate::doc_lock::stable_ids_of(doc) {
        index.lock().unwrap().watch.rescan();
    }
    Ok(())
}

/// The canonical live node carrying `stable_id` in `doc`: the smallest
/// `TreeID` when peers created the id more than once.
pub(crate) fn lookup(doc: &LoroDoc, stable_id: &str) -> Option<TreeID> {
    index_of(doc).lock().unwrap().lookup(doc, stable_id)
}

pub(crate) fn snapshot(doc: &LoroDoc) -> BTreeMap<String, Vec<TreeID>> {
    let index = index_of(doc);
    let mut index = index.lock().unwrap();
    index.refresh(doc, &doc.get_tree(TREE_NAME));
    index
        .live
        .iter()
        .map(|(sid, nodes)| (sid.clone(), nodes.clone()))
        .collect()
}

pub(crate) fn stats(doc: &LoroDoc) -> StableIdIndexStats {
    index_of(doc).lock().unwrap().stats
}

fn index_of(doc: &LoroDoc) -> StableIds {
    crate::doc_lock::stable_ids_of(doc)
        .expect("a stable-id lookup on a LoroDoc no LoroDocument wraps: wrap it first")
}

impl StableIdIndex {
    fn lookup(&mut self, doc: &LoroDoc, stable_id: &str) -> Option<TreeID> {
        let tree = doc.get_tree(TREE_NAME);
        self.refresh(doc, &tree);
        // An open batch's uncommitted delete or id rewrite is heard at its
        // commit; until then its node is skipped here.
        let found = self.live.get(stable_id).and_then(|nodes| {
            nodes
                .iter()
                .copied()
                .find(|&node| carries(&tree, node, stable_id))
        });
        if found.is_some() {
            return found;
        }
        if doc.get_pending_txn_len() > 0 {
            return self.scan_open_batch(&tree, stable_id);
        }
        None
    }

    /// The pending ops reach the index only at their commit, so the tree
    /// answers; the carriers found are noted for the next drain.
    fn scan_open_batch(&mut self, tree: &loro::LoroTree, stable_id: &str) -> Option<TreeID> {
        let nodes = tree.get_nodes(false);
        self.stats.open_batch_scans += 1;
        self.stats.nodes_visited += nodes.len() as u64;
        let found = carriers(tree, nodes, stable_id);
        for &node in &found {
            self.watch.note(node);
        }
        found.into_iter().min()
    }

    fn refresh(&mut self, doc: &LoroDoc, tree: &loro::LoroTree) {
        if self.watch.watch(doc, tree) {
            self.built = false;
        }
        match self.watch.drain() {
            Drained::Nodes(nodes) if self.built => self.apply(tree, nodes),
            Drained::Nodes(_) | Drained::Rescan => self.build(tree),
        }
        #[cfg(debug_assertions)]
        self.check(doc, tree);
    }

    /// Checks the index against the tree at every node the doc's history
    /// changed since the last check, after a build at every node. The changed
    /// nodes come from the oplog, so a change the watch never heard is checked
    /// too; ops of an open batch are checked after their commit.
    #[cfg(debug_assertions)]
    fn check(&mut self, doc: &LoroDoc, tree: &loro::LoroTree) {
        if doc.get_pending_txn_len() > 0 {
            return;
        }
        let now = doc.state_frontiers();
        let nodes = match &self.checked {
            Some(checked) if *checked == now => return,
            Some(checked) => subtrees(tree, changed_nodes(doc, tree, checked, &now)),
            None => tree
                .get_nodes(false)
                .into_iter()
                .map(|node| node.id)
                .collect(),
        };
        for &node in &nodes {
            let carried = carried(tree, node);
            let indexed = self.by_node.get(&node);
            let listed = indexed.is_none_or(|sid| {
                self.live
                    .get(sid)
                    .is_some_and(|carriers| carriers.binary_search(&node).is_ok())
            });
            assert!(
                indexed == carried.as_ref() && listed,
                "the stable-id index holds {indexed:?} for {node:?}, but the tree holds \
                 {carried:?}: a committed change to the tree did not reach the index"
            );
        }
        if self.checked.is_none() {
            let carriers = nodes
                .iter()
                .filter(|&&n| carried(tree, n).is_some())
                .count();
            assert_eq!(
                self.by_node.len(),
                carriers,
                "the stable-id index holds nodes the tree does not carry an id at"
            );
        }
        self.checked = Some(now);
    }

    fn build(&mut self, tree: &loro::LoroTree) {
        self.live.clear();
        self.by_node.clear();
        for node in tree.get_nodes(false) {
            self.stats.nodes_visited += 1;
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            if let LiveNode::Settled(sid) = classify(tree, node.id) {
                self.insert(sid, node.id);
            }
        }
        self.built = true;
        self.stats.full_builds += 1;
        #[cfg(debug_assertions)]
        {
            self.checked = None;
        }
    }

    /// Re-reads each touched node and its subtree: a delete reports only the
    /// subtree root, and a create or move can carry a subtree.
    fn apply(&mut self, tree: &loro::LoroTree, touched: Vec<TreeID>) {
        let mut seen = HashSet::new();
        let mut queue = touched;
        while let Some(node) = queue.pop() {
            if !seen.insert(node) {
                continue;
            }
            self.stats.nodes_visited += 1;
            self.forget(node);
            if let Some(sid) = carried(tree, node) {
                self.insert(sid, node);
            }
            queue.extend(tree.children(node).into_iter().flatten());
        }
    }

    fn insert(&mut self, sid: String, node: TreeID) {
        let nodes = self.live.entry(sid.clone()).or_default();
        if let Err(at) = nodes.binary_search(&node) {
            nodes.insert(at, node);
        }
        self.by_node.insert(node, sid);
    }

    fn forget(&mut self, node: TreeID) {
        let Some(sid) = self.by_node.remove(&node) else {
            return;
        };
        let nodes = self
            .live
            .get_mut(&sid)
            .expect("by_node and live name the same ids");
        nodes.retain(|&n| n != node);
        if nodes.is_empty() {
            self.live.remove(&sid);
        }
    }
}

fn carries(tree: &loro::LoroTree, node: TreeID, stable_id: &str) -> bool {
    carried(tree, node).is_some_and(|sid| sid == stable_id)
}

/// The id `node` carries while it is live and settled.
fn carried(tree: &loro::LoroTree, node: TreeID) -> Option<String> {
    if tree.is_node_deleted(&node).unwrap_or(true) {
        return None;
    }
    match classify(tree, node) {
        LiveNode::Settled(sid) => Some(sid),
        LiveNode::HalfBorn | LiveNode::MetaUnreadable => None,
    }
}

/// `roots` and all their descendants, each once.
#[cfg(debug_assertions)]
fn subtrees(tree: &loro::LoroTree, roots: Vec<TreeID>) -> Vec<TreeID> {
    let mut seen = HashSet::new();
    let mut queue = roots;
    let mut nodes = Vec::new();
    while let Some(node) = queue.pop() {
        if seen.insert(node) {
            nodes.push(node);
            queue.extend(tree.children(node).into_iter().flatten());
        }
    }
    nodes
}

/// The targets of the tree ops and the owners of the `STABLE_ID` meta writes
/// between `from` and `to`, in either direction.
#[cfg(debug_assertions)]
fn changed_nodes(
    doc: &LoroDoc,
    tree: &loro::LoroTree,
    from: &loro::Frontiers,
    to: &loro::Frontiers,
) -> Vec<TreeID> {
    use loro::JsonMapOp;
    use loro::JsonOpContent;
    use loro::JsonTreeOp;

    let tree_container = loro::ContainerTrait::id(tree);
    let spans = doc.find_id_spans_between(from, to);
    spans
        .retreat
        .iter()
        .chain(spans.forward.iter())
        .flat_map(|(&peer, span)| {
            doc.export_json_in_id_span(loro::IdSpan::new(peer, span.start, span.end))
        })
        .flat_map(|change| change.ops)
        .filter_map(|op| match op.content {
            JsonOpContent::Tree(
                JsonTreeOp::Create { target, .. }
                | JsonTreeOp::Move { target, .. }
                | JsonTreeOp::Delete { target },
            ) if op.container == tree_container => Some(target),
            JsonOpContent::Map(JsonMapOp::Insert { key, .. } | JsonMapOp::Delete { key })
                if key == STABLE_ID =>
            {
                meta_owner(&op.container)
            }
            _ => None,
        })
        .collect()
}

/// The node whose meta map `container` is, when it can be one.
#[cfg(debug_assertions)]
fn meta_owner(container: &loro::ContainerID) -> Option<TreeID> {
    match *container {
        loro::ContainerID::Normal {
            peer,
            counter,
            container_type: loro::ContainerType::Map,
        } => Some(TreeID { peer, counter }),
        _ => None,
    }
}

fn carriers(tree: &loro::LoroTree, nodes: Vec<loro::TreeNode>, stable_id: &str) -> Vec<TreeID> {
    nodes
        .into_iter()
        .filter(|node| {
            !matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            )
        })
        .filter(
            |node| matches!(classify(tree, node.id), LiveNode::Settled(sid) if sid == stable_id),
        )
        .map(|node| node.id)
        .collect()
}
