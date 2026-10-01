//! The nodes of a doc's block tree that changed since a derived holder last
//! looked, heard from one Loro subscription on the tree container.
//!
//! What the subscription hears is pinned by `tree_event_delivery_probe`: local
//! commits (also a failed batch's flush), imports, checkout and `revert_to` all
//! report the target of every create, move and delete — a subtree delete only
//! its root — and every meta-key write with a path that names the node.

use std::sync::Arc;
use std::sync::Mutex;

use loro::TreeID;

/// Past this many touched nodes a drain reports a full rescan instead.
const TOUCHED_LIMIT: usize = 1024;

/// The callback only pushes into a leaf mutex: it runs on the committing
/// thread, and loro's subscriber set is not panic-safe.
pub(crate) struct TreeWatch {
    /// A meta write of one of these keys asks for a full rescan.
    rescan_keys: &'static [&'static str],
    /// A meta write of one of these keys touches the node whose meta it is.
    touch_keys: &'static [&'static str],
    /// Address of the `LoroDoc` the subscription watches.
    doc: usize,
    subscription: Option<loro::Subscription>,
    touched: Arc<Mutex<Touched>>,
}

#[derive(Default)]
struct Touched {
    nodes: Vec<TreeID>,
    rescan: bool,
}

impl Touched {
    fn extend(&mut self, nodes: impl IntoIterator<Item = TreeID>) {
        if self.rescan {
            return;
        }
        self.nodes.extend(nodes);
        if self.nodes.len() > TOUCHED_LIMIT {
            self.nodes = Vec::new();
            self.rescan = true;
        }
    }
}

pub(crate) enum Drained {
    /// The targets of the tree events since the last drain; a delete reports
    /// only the subtree root, a create or move can carry a subtree.
    Nodes(Vec<TreeID>),
    Rescan,
}

impl TreeWatch {
    pub(crate) fn new(
        rescan_keys: &'static [&'static str],
        touch_keys: &'static [&'static str],
    ) -> Self {
        Self {
            rescan_keys,
            touch_keys,
            doc: 0,
            subscription: None,
            touched: Arc::default(),
        }
    }

    /// Subscribes to `tree` of `doc` unless this watch already does. Returns
    /// `true` when it subscribed now: changes before that were not heard.
    pub(crate) fn watch(&mut self, doc: &loro::LoroDoc, tree: &loro::LoroTree) -> bool {
        let address = std::ptr::from_ref(doc) as usize;
        if self.subscription.is_some() && self.doc == address {
            return false;
        }
        let touched = self.touched.clone();
        let rescan_keys = self.rescan_keys;
        let touch_keys = self.touch_keys;
        self.subscription = Some(doc.subscribe(
            &loro::ContainerTrait::id(tree),
            Arc::new(move |event| {
                let mut touched = touched.lock().unwrap();
                for diff in &event.events {
                    if touched.rescan {
                        return;
                    }
                    match &diff.diff {
                        loro::event::Diff::Tree(tree_diff) => {
                            touched.extend(tree_diff.diff.iter().map(|item| item.target));
                        }
                        loro::event::Diff::Map(map)
                            if map.updated.keys().any(|key| rescan_keys.contains(&&**key)) =>
                        {
                            touched.nodes = Vec::new();
                            touched.rescan = true;
                        }
                        loro::event::Diff::Map(map)
                            if map.updated.keys().any(|key| touch_keys.contains(&&**key)) =>
                        {
                            // A node's own meta map is the diff target exactly
                            // when its path ends in the node.
                            if let Some((_, loro::Index::Node(node))) = diff.path.last() {
                                touched.extend([*node]);
                            }
                        }
                        _ => {}
                    }
                }
            }),
        ));
        self.doc = address;
        true
    }

    /// Touches `node` without an event: a change the doc reports only at the
    /// next commit.
    pub(crate) fn note(&self, node: TreeID) {
        self.touched.lock().unwrap().extend([node]);
    }

    pub(crate) fn drain(&self) -> Drained {
        let touched = std::mem::take(&mut *self.touched.lock().unwrap());
        if touched.rescan {
            Drained::Rescan
        } else {
            Drained::Nodes(touched.nodes)
        }
    }
}
