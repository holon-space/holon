//! Incremental tree-to-flat-list data structure.
//!
//! `MutableTree` maintains a tree of nodes (keyed by ID, with parent_id and
//! sort_key) and projects it as a DFS-ordered `MutableVec`. Mutations (insert,
//! update, remove) emit precise `VecDiff` events — the common case of a content
//! edit produces a single `VecDiff::UpdateAt`.
//!
//! Each node's widget is wrapped in a `TreeItem(depth, has_children)` before
//! being written to the flat output.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use futures_signals::signal_vec::MutableVec;

use crate::reactive_view_model::ReactiveViewModel;
use holon_api::render_eval::sort_value;
use holon_api::Value;

/// A node in the sort order. `Ord` sorts by (sort_key, id) so siblings
/// appear in the right order.
#[derive(Debug, Clone, Eq, PartialEq)]
struct SortedChild {
    sort_key_bits: u64,
    id: String,
}

impl SortedChild {
    fn new(sort_key: f64, id: String) -> Self {
        Self {
            sort_key_bits: sort_key.to_bits(),
            id,
        }
    }

    fn sort_key(&self) -> f64 {
        f64::from_bits(self.sort_key_bits)
    }
}

impl Ord for SortedChild {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key()
            .partial_cmp(&other.sort_key())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for SortedChild {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct TreeNode {
    parent_id: Option<String>,
    sort_key: f64,
    depth: usize,
    /// The raw widget (before TreeItem wrapping).
    widget: Arc<ReactiveViewModel>,
}

/// Incremental tree that maintains a DFS-ordered `MutableVec`.
///
/// # Usage
/// ```ignore
/// let tree = MutableTree::new(collection_items.clone());
/// tree.insert("a", None, 0.0, widget_a);
/// tree.insert("b", Some("a"), 0.0, widget_b);
/// // collection_items now has [TreeItem(a, depth=0), TreeItem(b, depth=1)]
/// ```
pub struct MutableTree {
    nodes: HashMap<String, TreeNode>,
    /// parent_id → sorted children. `None` key = root nodes.
    children: HashMap<Option<String>, BTreeSet<SortedChild>>,
    /// Current DFS order — mirrors indices in `flat`.
    flat_order: Vec<String>,
    /// id → index in flat_order/flat. O(1) position lookups.
    flat_index: HashMap<String, usize>,
    /// The output MutableVec that CollectionView subscribes to.
    flat: MutableVec<Arc<ReactiveViewModel>>,
}

impl MutableTree {
    /// Create a new MutableTree that writes to the given MutableVec.
    pub fn new(flat: MutableVec<Arc<ReactiveViewModel>>) -> Self {
        Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            flat_order: Vec::new(),
            flat_index: HashMap::new(),
            flat,
        }
    }

    /// Snapshot the current flat order as IDs (for testing).
    pub fn flat_ids(&self) -> Vec<String> {
        self.flat_order.clone()
    }

    /// Snapshot the current flat items (for testing).
    pub fn flat_snapshot(&self) -> Vec<(String, usize, bool)> {
        self.flat_order
            .iter()
            .map(|id| {
                let node = &self.nodes[id];
                let has_children = self
                    .children
                    .get(&Some(id.clone()))
                    .map_or(false, |c| !c.is_empty());
                (id.clone(), node.depth, has_children)
            })
            .collect()
    }

    /// Insert a new node. If `parent_id` references a non-existent node, treats as root.
    pub fn insert(
        &mut self,
        id: String,
        parent_id: Option<String>,
        sort_key: f64,
        widget: Arc<ReactiveViewModel>,
    ) {
        // Treat as root if parent doesn't exist in the tree.
        let effective_parent = parent_id.filter(|pid| self.nodes.contains_key(pid));

        let depth = effective_parent
            .as_ref()
            .map_or(0, |pid| self.nodes[pid].depth + 1);

        self.nodes.insert(
            id.clone(),
            TreeNode {
                parent_id: effective_parent.clone(),
                sort_key,
                depth,
                widget: widget.clone(),
            },
        );

        let sorted = SortedChild::new(sort_key, id.clone());
        self.children
            .entry(effective_parent.clone())
            .or_default()
            .insert(sorted);

        let parent_had_children_before = self
            .children
            .get(&effective_parent)
            .map_or(false, |c| c.len() > 1);

        let pos = self.compute_dfs_position(&id, &effective_parent);

        let has_children = self
            .children
            .get(&Some(id.clone()))
            .map_or(false, |c| !c.is_empty());
        let wrapped = wrap_tree_item(&widget, depth, has_children);

        self.flat_insert(pos, id.clone());
        self.flat.lock_mut().insert_cloned(pos, Arc::new(wrapped));

        if let Some(ref pid) = effective_parent {
            if !parent_had_children_before {
                self.update_has_children(pid);
            }
        }
    }

    /// Update a node's data. If parent_id or sort_key changed, moves the node.
    pub fn update(
        &mut self,
        id: &str,
        parent_id: Option<String>,
        sort_key: f64,
        widget: Arc<ReactiveViewModel>,
    ) {
        let Some(old) = self.nodes.get(id) else {
            return;
        };

        // Normalize: treat missing parent as root, same as insert.
        let effective_parent = parent_id.filter(|pid| pid != id && self.nodes.contains_key(pid));
        let structure_changed = old.parent_id != effective_parent || old.sort_key != sort_key;

        if structure_changed {
            let id_owned = id.to_string();
            self.remove(&id_owned);
            self.insert(id_owned, effective_parent, sort_key, widget);
        } else {
            let pos = self.pos_of(id).expect("node in flat_index");
            let node = self.nodes.get_mut(id).expect("node in nodes map");
            node.widget = widget;
            let has_children = self
                .children
                .get(&Some(id.to_string()))
                .map_or(false, |c| !c.is_empty());
            let wrapped = wrap_tree_item(&node.widget, node.depth, has_children);
            self.flat.lock_mut().set_cloned(pos, Arc::new(wrapped));
        }
    }

    /// Remove a node and all its descendants.
    pub fn remove(&mut self, id: &str) {
        let Some(pos) = self.pos_of(id) else {
            return;
        };

        let subtree_end = self.subtree_end(pos);

        // Remove from flat in reverse order so MutableVec indices stay valid.
        {
            let mut lock = self.flat.lock_mut();
            for i in (pos..subtree_end).rev() {
                lock.remove(i);
            }
        }
        let subtree_ids: Vec<String> = self.flat_order.drain(pos..subtree_end).collect();
        self.rebuild_flat_index();

        // Clean up internal structures.
        let parent_id = self.nodes.get(id).and_then(|n| n.parent_id.clone());
        for sub_id in &subtree_ids {
            if let Some(node) = self.nodes.remove(sub_id) {
                if let Some(siblings) = self.children.get_mut(&node.parent_id) {
                    siblings.remove(&SortedChild::new(node.sort_key, sub_id.clone()));
                    if siblings.is_empty() {
                        self.children.remove(&node.parent_id);
                    }
                }
            }
            self.children.remove(&Some(sub_id.clone()));
        }

        if let Some(ref pid) = parent_id {
            if !self
                .children
                .get(&Some(pid.clone()))
                .map_or(false, |c| !c.is_empty())
            {
                self.update_has_children(pid);
            }
        }
    }

    /// Rebuild from scratch. Emits a single `VecDiff::Replace`.
    pub fn rebuild(&mut self, entries: Vec<(String, Option<String>, f64, Arc<ReactiveViewModel>)>) {
        self.nodes.clear();
        self.children.clear();
        self.flat_order.clear();
        self.flat_index.clear();

        let all_ids: std::collections::HashSet<&str> =
            entries.iter().map(|(id, _, _, _)| id.as_str()).collect();

        for (id, parent_id, sort_key, widget) in &entries {
            let effective_parent = parent_id
                .as_ref()
                .filter(|pid| all_ids.contains(pid.as_str()))
                .cloned();

            self.nodes.insert(
                id.clone(),
                TreeNode {
                    parent_id: effective_parent.clone(),
                    sort_key: *sort_key,
                    depth: 0,
                    widget: widget.clone(),
                },
            );

            let sorted = SortedChild::new(*sort_key, id.clone());
            self.children
                .entry(effective_parent)
                .or_default()
                .insert(sorted);
        }
        drop(all_ids);

        self.compute_depths();

        let roots: Vec<String> = self
            .children
            .get(&None)
            .map_or(Vec::new(), |c| c.iter().map(|sc| sc.id.clone()).collect());

        let mut order = Vec::new();
        self.walk_dfs_into(&roots, &mut order);
        self.flat_order = order;
        self.rebuild_flat_index();

        let items: Vec<Arc<ReactiveViewModel>> = self
            .flat_order
            .iter()
            .map(|id| {
                let node = &self.nodes[id];
                let has_children = self
                    .children
                    .get(&Some(id.clone()))
                    .map_or(false, |c| !c.is_empty());
                Arc::new(wrap_tree_item(&node.widget, node.depth, has_children))
            })
            .collect();

        self.flat.lock_mut().replace_cloned(items);
    }

    // ── Private helpers ─────────────────────────────────────────────────

    fn compute_depths(&mut self) {
        let roots: Vec<String> = self
            .children
            .get(&None)
            .map_or(Vec::new(), |c| c.iter().map(|sc| sc.id.clone()).collect());

        let mut stack: Vec<(String, usize)> = roots.into_iter().map(|id| (id, 0)).collect();
        while let Some((id, depth)) = stack.pop() {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.depth = depth;
            }
            if let Some(child_set) = self.children.get(&Some(id)) {
                for child in child_set.iter().rev() {
                    stack.push((child.id.clone(), depth + 1));
                }
            }
        }
    }

    fn walk_dfs_into(&self, ids: &[String], out: &mut Vec<String>) {
        for id in ids {
            out.push(id.clone());
            if let Some(child_set) = self.children.get(&Some(id.clone())) {
                let child_ids: Vec<String> = child_set.iter().map(|sc| sc.id.clone()).collect();
                self.walk_dfs_into(&child_ids, out);
            }
        }
    }

    /// O(1) position lookup via flat_index.
    fn pos_of(&self, id: &str) -> Option<usize> {
        self.flat_index.get(id).copied()
    }

    /// Insert an id into flat_order at `pos` and update flat_index.
    fn flat_insert(&mut self, pos: usize, id: String) {
        self.flat_order.insert(pos, id.clone());
        // Shift all indices >= pos
        for (_, idx) in self.flat_index.iter_mut() {
            if *idx >= pos {
                *idx += 1;
            }
        }
        self.flat_index.insert(id, pos);
    }

    /// Rebuild flat_index from flat_order. Used after drain operations.
    fn rebuild_flat_index(&mut self) {
        self.flat_index.clear();
        for (i, id) in self.flat_order.iter().enumerate() {
            self.flat_index.insert(id.clone(), i);
        }
    }

    /// Compute where a new node should go in the flat list.
    fn compute_dfs_position(&self, id: &str, parent_id: &Option<String>) -> usize {
        let siblings = match self.children.get(parent_id) {
            Some(s) => s,
            None => return self.flat_order.len(),
        };

        // Find the sibling that comes right after us in sort order.
        let mut found_self = false;
        for sibling in siblings.iter() {
            if sibling.id == id {
                found_self = true;
                continue;
            }
            if found_self {
                if let Some(pos) = self.pos_of(&sibling.id) {
                    return pos;
                }
            }
        }

        // Last sibling — insert after previous sibling's subtree.
        let mut prev_sibling_id: Option<&str> = None;
        for sibling in siblings.iter() {
            if sibling.id == id {
                break;
            }
            prev_sibling_id = Some(&sibling.id);
        }

        if let Some(prev_id) = prev_sibling_id {
            if let Some(prev_pos) = self.pos_of(prev_id) {
                return self.subtree_end(prev_pos);
            }
        }

        // No previous sibling — insert right after parent.
        if let Some(pid) = parent_id {
            if let Some(parent_pos) = self.pos_of(pid) {
                return parent_pos + 1;
            }
        }

        self.flat_order.len()
    }

    /// Find the end of a node's subtree (exclusive) in flat_order.
    fn subtree_end(&self, pos: usize) -> usize {
        let node_depth = self.nodes[&self.flat_order[pos]].depth;
        for i in (pos + 1)..self.flat_order.len() {
            if self.nodes[&self.flat_order[i]].depth <= node_depth {
                return i;
            }
        }
        self.flat_order.len()
    }

    /// Re-emit a node's TreeItem wrapper (to update has_children flag).
    fn update_has_children(&self, id: &str) {
        let Some(pos) = self.pos_of(id) else {
            return;
        };
        let node = &self.nodes[id];
        let has_children = self
            .children
            .get(&Some(id.to_string()))
            .map_or(false, |c| !c.is_empty());
        let wrapped = wrap_tree_item(&node.widget, node.depth, has_children);
        self.flat.lock_mut().set_cloned(pos, Arc::new(wrapped));
    }
}

/// Wrap a widget in a TreeItem with the given depth and has_children flag.
fn wrap_tree_item(
    widget: &Arc<ReactiveViewModel>,
    depth: usize,
    has_children: bool,
) -> ReactiveViewModel {
    let mut props = std::collections::HashMap::new();
    props.insert("depth".to_string(), Value::Integer(depth as i64));
    props.insert("has_children".to_string(), Value::Boolean(has_children));
    ReactiveViewModel {
        children: vec![widget.clone()],
        data: futures_signals::signal::Mutable::new(widget.entity()).read_only(),
        ..ReactiveViewModel::from_widget("tree_item", props)
    }
}

/// Extract parent_id from a DataRow.
pub fn extract_parent_id(row: &HashMap<String, Value>) -> Option<String> {
    row.get("parent_id")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
}

/// Extract sort_key from a DataRow as f64.
pub fn extract_sort_key(row: &HashMap<String, Value>) -> f64 {
    let v = row.get("sequence").or_else(|| row.get("sort_key"));
    sort_value(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive_view_model::ReactiveViewModel;

    fn widget(name: &str) -> Arc<ReactiveViewModel> {
        Arc::new(ReactiveViewModel::text(name))
    }

    fn make_tree() -> (MutableTree, MutableVec<Arc<ReactiveViewModel>>) {
        let flat = MutableVec::new();
        let tree = MutableTree::new(flat.clone());
        (tree, flat)
    }

    #[test]
    fn insert_root_nodes() {
        let (mut tree, flat) = make_tree();
        tree.insert("a".into(), None, 0.0, widget("A"));
        tree.insert("b".into(), None, 1.0, widget("B"));

        assert_eq!(tree.flat_ids(), vec!["a", "b"]);
        assert_eq!(flat.lock_ref().len(), 2);
    }

    #[test]
    fn insert_child_computes_depth() {
        let (mut tree, _) = make_tree();
        tree.insert("root".into(), None, 0.0, widget("Root"));
        tree.insert("child".into(), Some("root".into()), 0.0, widget("Child"));

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], ("root".into(), 0, true));
        assert_eq!(snap[1], ("child".into(), 1, false));
    }

    #[test]
    fn insert_grandchild() {
        let (mut tree, _) = make_tree();
        tree.insert("a".into(), None, 0.0, widget("A"));
        tree.insert("b".into(), Some("a".into()), 0.0, widget("B"));
        tree.insert("c".into(), Some("b".into()), 0.0, widget("C"));

        let snap = tree.flat_snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].1, 0); // a depth=0
        assert_eq!(snap[1].1, 1); // b depth=1
        assert_eq!(snap[2].1, 2); // c depth=2
    }

    #[test]
    fn siblings_sorted_by_sort_key() {
        let (mut tree, _) = make_tree();
        tree.insert("root".into(), None, 0.0, widget("Root"));
        tree.insert("c".into(), Some("root".into()), 2.0, widget("C"));
        tree.insert("a".into(), Some("root".into()), 0.0, widget("A"));
        tree.insert("b".into(), Some("root".into()), 1.0, widget("B"));

        assert_eq!(tree.flat_ids(), vec!["root", "a", "b", "c"]);
    }

    #[test]
    fn insert_between_siblings_with_children() {
        let (mut tree, _) = make_tree();
        tree.insert("root".into(), None, 0.0, widget("Root"));
        tree.insert("s1".into(), Some("root".into()), 0.0, widget("S1"));
        tree.insert("s1c".into(), Some("s1".into()), 0.0, widget("S1-child"));
        tree.insert("s3".into(), Some("root".into()), 2.0, widget("S3"));
        // Insert s2 between s1 and s3
        tree.insert("s2".into(), Some("root".into()), 1.0, widget("S2"));

        assert_eq!(tree.flat_ids(), vec!["root", "s1", "s1c", "s2", "s3"]);
    }

    #[test]
    fn update_data_only() {
        let (mut tree, flat) = make_tree();
        tree.insert("a".into(), None, 0.0, widget("old"));

        tree.update("a", None, 0.0, widget("new"));

        assert_eq!(tree.flat_ids(), vec!["a"]);
        assert_eq!(flat.lock_ref().len(), 1);
    }

    #[test]
    fn update_reparent() {
        let (mut tree, _) = make_tree();
        tree.insert("a".into(), None, 0.0, widget("A"));
        tree.insert("b".into(), None, 1.0, widget("B"));
        tree.insert("c".into(), Some("a".into()), 0.0, widget("C"));

        // Move c from under a to under b
        tree.update("c", Some("b".into()), 0.0, widget("C"));

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], ("a".into(), 0, false)); // a lost its child
        assert_eq!(snap[1], ("b".into(), 0, true)); // b gained a child
        assert_eq!(snap[2], ("c".into(), 1, false)); // c under b
    }

    #[test]
    fn remove_leaf() {
        let (mut tree, flat) = make_tree();
        tree.insert("a".into(), None, 0.0, widget("A"));
        tree.insert("b".into(), None, 1.0, widget("B"));

        tree.remove("a");

        assert_eq!(tree.flat_ids(), vec!["b"]);
        assert_eq!(flat.lock_ref().len(), 1);
    }

    #[test]
    fn remove_subtree() {
        let (mut tree, _) = make_tree();
        tree.insert("root".into(), None, 0.0, widget("Root"));
        tree.insert("child".into(), Some("root".into()), 0.0, widget("Child"));
        tree.insert("grandchild".into(), Some("child".into()), 0.0, widget("GC"));
        tree.insert("other".into(), None, 1.0, widget("Other"));

        tree.remove("root");

        assert_eq!(tree.flat_ids(), vec!["other"]);
    }

    #[test]
    fn remove_updates_parent_has_children() {
        let (mut tree, _) = make_tree();
        tree.insert("parent".into(), None, 0.0, widget("Parent"));
        tree.insert("child".into(), Some("parent".into()), 0.0, widget("Child"));

        assert!(tree.flat_snapshot()[0].2); // has_children = true

        tree.remove("child");

        assert!(!tree.flat_snapshot()[0].2); // has_children = false
    }

    #[test]
    fn rebuild_from_scratch() {
        let (mut tree, flat) = make_tree();
        tree.insert("old".into(), None, 0.0, widget("Old"));

        tree.rebuild(vec![
            ("a".into(), None, 0.0, widget("A")),
            ("b".into(), Some("a".into()), 0.0, widget("B")),
            ("c".into(), None, 1.0, widget("C")),
        ]);

        assert_eq!(tree.flat_ids(), vec!["a", "b", "c"]);
        assert_eq!(flat.lock_ref().len(), 3);

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], ("a".into(), 0, true));
        assert_eq!(snap[1], ("b".into(), 1, false));
        assert_eq!(snap[2], ("c".into(), 0, false));
    }

    #[test]
    fn rebuild_ignores_missing_parents() {
        let (mut tree, _) = make_tree();
        tree.rebuild(vec![
            ("a".into(), Some("nonexistent".into()), 0.0, widget("A")),
            ("b".into(), Some("a".into()), 0.0, widget("B")),
        ]);

        let snap = tree.flat_snapshot();
        // "a" becomes a root because its parent doesn't exist in the dataset
        assert_eq!(snap[0], ("a".into(), 0, true));
        assert_eq!(snap[1], ("b".into(), 1, false));
    }
}
