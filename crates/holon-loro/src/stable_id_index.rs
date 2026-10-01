//! `stable id -> TreeID` for every live, settled node of one doc's block tree.
//!
//! Complete: a miss means no live node carries the id. The index lives in the
//! doc's registry entry beside its boundary lock, so every wrapper of one
//! `LoroDoc` shares it and a new doc starts with an empty one.
//!
//! It hears committed changes through a [`TreeWatch`]. An open write batch's
//! own ops are not committed yet; [`write_stable_id`] notes the node it writes,
//! so a lookup inside the batch sees the batch's new ids.

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
    /// Nodes read by full builds and by drains of touched subtrees.
    pub nodes_visited: u64,
}

pub(crate) struct StableIdIndex {
    built: bool,
    /// Every live carrier of an id, ascending; the first is canonical.
    live: HashMap<String, Vec<TreeID>>,
    by_node: HashMap<TreeID, String>,
    watch: TreeWatch,
    stats: StableIdIndexStats,
}

impl Default for StableIdIndex {
    fn default() -> Self {
        Self {
            built: false,
            live: HashMap::new(),
            by_node: HashMap::new(),
            watch: TreeWatch::new(&[], &[STABLE_ID]),
            stats: StableIdIndexStats::default(),
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
        #[cfg(debug_assertions)]
        if found.is_none() {
            let scanned = scan(&tree).remove(stable_id);
            assert!(
                scanned.is_none(),
                "the stable-id index misses `{stable_id}`, but the tree holds it at {scanned:?}: \
                 a change to the tree did not reach the index"
            );
        }
        found
    }

    fn refresh(&mut self, doc: &LoroDoc, tree: &loro::LoroTree) {
        if self.watch.watch(doc, tree) {
            self.built = false;
        }
        match self.watch.drain() {
            Drained::Nodes(nodes) if self.built => self.apply(tree, nodes),
            Drained::Nodes(_) | Drained::Rescan => self.build(tree),
        }
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
            if !tree.is_node_deleted(&node).unwrap_or(true)
                && let LiveNode::Settled(sid) = classify(tree, node)
            {
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
    !tree.is_node_deleted(&node).unwrap_or(true)
        && matches!(classify(tree, node), LiveNode::Settled(sid) if sid == stable_id)
}

#[cfg(debug_assertions)]
fn scan(tree: &loro::LoroTree) -> HashMap<String, Vec<TreeID>> {
    let mut found: HashMap<String, Vec<TreeID>> = HashMap::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        if let LiveNode::Settled(sid) = classify(tree, node.id) {
            found.entry(sid).or_default().push(node.id);
        }
    }
    found
}
