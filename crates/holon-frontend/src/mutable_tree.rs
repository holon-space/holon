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
use holon_api::{EntityUri, Value};

/// A node in the sort order. `Ord` sorts by (sort_key, id) so siblings
/// appear in the right order. `sort_key` is a string whose lexicographic
/// byte order matches the desired sort order (FractionalIndex hex strings
/// or zero-padded numeric values from [`holon_api::render_eval::sort_value`]).
#[derive(Debug, Clone, Eq, PartialEq)]
struct SortedChild {
    sort_key: String,
    id: EntityUri,
}

impl SortedChild {
    fn new(sort_key: String, id: EntityUri) -> Self {
        Self { sort_key, id }
    }
}

impl Ord for SortedChild {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key
            .cmp(&other.sort_key)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for SortedChild {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct TreeNode {
    /// Effective parent: the stated parent if it is present in the tree, else
    /// `None` (rendered as a root until the real parent arrives).
    parent_id: Option<EntityUri>,
    /// Originally-requested parent, retained even while absent so a later
    /// insert of that parent can adopt this orphan (the row stream is keyed by
    /// id, not topological, so children can arrive before their parent).
    stated_parent: Option<EntityUri>,
    sort_key: String,
    depth: usize,
    /// The raw widget (before TreeItem wrapping).
    widget: Arc<ReactiveViewModel>,
    /// Rule-override props (`role`, `show_bullet`, `show_chevron`, …) merged
    /// into the TreeItem wrapper — streaming twin of `flat_tree_items`.
    overrides: HashMap<String, Value>,
}

/// Incremental tree that maintains a DFS-ordered `MutableVec`.
///
/// # Usage
/// ```ignore
/// let tree = MutableTree::new(collection_items.clone());
/// tree.insert("a", None, "0.0".into(), widget_a, HashMap::new());
/// tree.insert("b", Some("a"), "0.0".into(), widget_b, HashMap::new());
/// // collection_items now has [TreeItem(a, depth=0), TreeItem(b, depth=1)]
/// ```
pub struct MutableTree {
    nodes: HashMap<EntityUri, TreeNode>,
    /// parent_id → sorted children. `None` key = root nodes.
    children: HashMap<Option<EntityUri>, BTreeSet<SortedChild>>,
    /// Current DFS order — mirrors indices in `flat`.
    flat_order: Vec<EntityUri>,
    /// id → index in flat_order/flat. O(1) position lookups.
    flat_index: HashMap<EntityUri, usize>,
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
    pub fn flat_ids(&self) -> Vec<EntityUri> {
        self.flat_order.clone()
    }

    /// Snapshot the current flat items (for testing).
    pub fn flat_snapshot(&self) -> Vec<(EntityUri, usize, bool)> {
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
    ///
    /// Returns the ids of previously-stranded nodes (and their descendants)
    /// this insert adopted — their depth changed, so the caller must
    /// re-interpret their rows (depth-dependent `rules:` outcomes are baked
    /// into the widget at interpret time).
    pub fn insert(
        &mut self,
        id: EntityUri,
        parent_id: Option<EntityUri>,
        sort_key: String,
        widget: Arc<ReactiveViewModel>,
        overrides: HashMap<String, Value>,
    ) -> Vec<EntityUri> {
        // Treat as root if parent doesn't exist in the tree (yet) — but
        // remember the stated parent so `adopt_orphans` can pull this node
        // under it once the real parent arrives.
        let stated_parent = parent_id;
        let effective_parent = stated_parent
            .as_ref()
            .filter(|pid| self.nodes.contains_key(pid))
            .cloned();

        let depth = effective_parent
            .as_ref()
            .map_or(0, |pid| self.nodes[pid].depth + 1);

        self.nodes.insert(
            id.clone(),
            TreeNode {
                parent_id: effective_parent.clone(),
                stated_parent,
                sort_key: sort_key.clone(),
                depth,
                widget: widget.clone(),
                overrides: overrides.clone(),
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
        let wrapped = wrap_tree_item(&widget, depth, has_children, &overrides);

        self.flat_insert(pos, id.clone());
        self.flat.lock_mut().insert_cloned(pos, Arc::new(wrapped));

        if let Some(ref pid) = effective_parent {
            if !parent_had_children_before {
                self.update_has_children(pid);
            }
        }

        // Adopt any nodes that arrived earlier stating `id` as their parent but
        // were stranded as roots. Common case (in-order / no orphans) is a
        // cheap no-op; only an actual out-of-order arrival triggers the reflow.
        self.adopt_orphans(&id)
    }

    /// Re-parent root nodes whose stated parent is `parent` (now present) under
    /// it, then reflow. Recurses so a chain of out-of-order arrivals
    /// (grandchild before child before parent) fully resolves.
    ///
    /// Returns every node whose depth changed (adopted roots + their whole
    /// subtrees) so the driver can re-interpret their rows.
    fn adopt_orphans(&mut self, parent: &EntityUri) -> Vec<EntityUri> {
        let orphans: Vec<EntityUri> = match self.children.get(&None) {
            Some(roots) => roots
                .iter()
                .filter(|sc| {
                    self.nodes
                        .get(&sc.id)
                        .and_then(|n| n.stated_parent.as_ref())
                        == Some(parent)
                })
                .map(|sc| sc.id.clone())
                .collect(),
            None => Vec::new(),
        };
        if orphans.is_empty() {
            return Vec::new();
        }
        for orphan in &orphans {
            let sort_key = self.nodes[orphan].sort_key.clone();
            if let Some(roots) = self.children.get_mut(&None) {
                roots.remove(&SortedChild::new(sort_key.clone(), orphan.clone()));
                if roots.is_empty() {
                    self.children.remove(&None);
                }
            }
            self.children
                .entry(Some(parent.clone()))
                .or_default()
                .insert(SortedChild::new(sort_key, orphan.clone()));
            self.nodes
                .get_mut(orphan)
                .expect("orphan in nodes")
                .parent_id = Some(parent.clone());
        }
        // Recurse before reflowing so depths are computed against the final
        // structure in one pass.
        for orphan in &orphans {
            self.adopt_orphans(orphan);
        }
        self.reflow();
        // Post-restructure DFS over the adopted roots covers everything the
        // recursion pulled in, plus descendants that were already attached.
        let mut affected = Vec::new();
        self.walk_dfs_into(&orphans, &mut affected);
        affected
    }

    /// Recompute depths + DFS `flat_order` from the current `nodes`/`children`
    /// structure and republish the flat `MutableVec` (single `Replace`). Used
    /// after structural surgery (`adopt_orphans`) that the incremental
    /// flat-order maintenance can't express as local inserts.
    fn reflow(&mut self) {
        self.compute_depths();
        let roots: Vec<EntityUri> = self
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
                Arc::new(wrap_tree_item(
                    &node.widget,
                    node.depth,
                    has_children,
                    &node.overrides,
                ))
            })
            .collect();
        self.flat.lock_mut().replace_cloned(items);
    }

    /// Update a node's data. If parent_id or sort_key changed, moves the node
    /// (with its whole subtree) via children-map surgery + reflow.
    pub fn update(
        &mut self,
        id: &EntityUri,
        parent_id: Option<EntityUri>,
        sort_key: String,
        widget: Arc<ReactiveViewModel>,
        overrides: HashMap<String, Value>,
    ) {
        let Some(old) = self.nodes.get(id) else {
            panic!("MutableTree::update on unknown node {id}");
        };

        // Normalize: treat missing parent as root, same as insert. A parent
        // inside the node's own subtree (transient state while concurrent
        // moves converge) would create a cycle — treat it as root too, with
        // the stated parent retained; the next structural rebuild resolves it.
        let stated_parent = parent_id.clone();
        let effective_parent = parent_id.filter(|pid| {
            pid != id && self.nodes.contains_key(pid) && !self.is_in_subtree_of(pid, id)
        });
        if stated_parent.is_some()
            && effective_parent.is_none()
            && stated_parent.as_ref() != Some(id)
        {
            if let Some(sp) = &stated_parent {
                if self.nodes.contains_key(sp) {
                    tracing::warn!(
                        "MutableTree::update({id}): stated parent {sp} is inside the node's own \
                         subtree; rendering as root until convergence"
                    );
                }
            }
        }
        let structure_changed = old.parent_id != effective_parent || old.sort_key != sort_key;

        if structure_changed {
            let old_parent = old.parent_id.clone();
            let old_sort_key = old.sort_key.clone();
            if let Some(siblings) = self.children.get_mut(&old_parent) {
                siblings.remove(&SortedChild::new(old_sort_key, id.clone()));
                if siblings.is_empty() {
                    self.children.remove(&old_parent);
                }
            }
            self.children
                .entry(effective_parent.clone())
                .or_default()
                .insert(SortedChild::new(sort_key.clone(), id.clone()));
            let node = self.nodes.get_mut(id).expect("node in nodes map");
            node.parent_id = effective_parent;
            node.stated_parent = stated_parent;
            node.sort_key = sort_key;
            node.widget = widget;
            node.overrides = overrides;
            self.reflow();
        } else {
            let pos = self.pos_of(id).expect("node in flat_index");
            let node = self.nodes.get_mut(id).expect("node in nodes map");
            node.widget = widget;
            node.overrides = overrides;
            let has_children = self
                .children
                .get(&Some(id.clone()))
                .map_or(false, |c| !c.is_empty());
            let wrapped = wrap_tree_item(&node.widget, node.depth, has_children, &node.overrides);
            self.flat.lock_mut().set_cloned(pos, Arc::new(wrapped));
        }
    }

    /// Remove a node and all its descendants.
    pub fn remove(&mut self, id: &EntityUri) {
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
        let subtree_ids: Vec<EntityUri> = self.flat_order.drain(pos..subtree_end).collect();
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
    pub fn rebuild(
        &mut self,
        entries: Vec<(
            EntityUri,
            Option<EntityUri>,
            String,
            Arc<ReactiveViewModel>,
            HashMap<String, Value>,
        )>,
    ) {
        self.nodes.clear();
        self.children.clear();
        self.flat_order.clear();
        self.flat_index.clear();

        let all_ids: std::collections::HashSet<&EntityUri> =
            entries.iter().map(|(id, _, _, _, _)| id).collect();

        for (id, parent_id, sort_key, widget, overrides) in &entries {
            let effective_parent = parent_id
                .as_ref()
                .filter(|pid| all_ids.contains(pid))
                .cloned();

            self.nodes.insert(
                id.clone(),
                TreeNode {
                    parent_id: effective_parent.clone(),
                    stated_parent: parent_id.clone(),
                    sort_key: sort_key.clone(),
                    depth: 0,
                    widget: widget.clone(),
                    overrides: overrides.clone(),
                },
            );

            let sorted = SortedChild::new(sort_key.clone(), id.clone());
            self.children
                .entry(effective_parent)
                .or_default()
                .insert(sorted);
        }
        drop(all_ids);

        self.reflow();
    }

    // ── Private helpers ─────────────────────────────────────────────────

    fn compute_depths(&mut self) {
        let roots: Vec<EntityUri> = self
            .children
            .get(&None)
            .map_or(Vec::new(), |c| c.iter().map(|sc| sc.id.clone()).collect());

        let mut stack: Vec<(EntityUri, usize)> = roots.into_iter().map(|id| (id, 0)).collect();
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

    fn walk_dfs_into(&self, ids: &[EntityUri], out: &mut Vec<EntityUri>) {
        for id in ids {
            out.push(id.clone());
            if let Some(child_set) = self.children.get(&Some(id.clone())) {
                let child_ids: Vec<EntityUri> = child_set.iter().map(|sc| sc.id.clone()).collect();
                self.walk_dfs_into(&child_ids, out);
            }
        }
    }

    /// O(1) position lookup via flat_index.
    fn pos_of(&self, id: &EntityUri) -> Option<usize> {
        self.flat_index.get(id).copied()
    }

    /// True if `id` lies inside `ancestor`'s subtree (including `id == ancestor`).
    fn is_in_subtree_of(&self, id: &EntityUri, ancestor: &EntityUri) -> bool {
        let mut current = Some(id);
        while let Some(cur) = current {
            if cur == ancestor {
                return true;
            }
            current = self.nodes.get(cur).and_then(|n| n.parent_id.as_ref());
        }
        false
    }

    /// Insert an id into flat_order at `pos` and update flat_index.
    fn flat_insert(&mut self, pos: usize, id: EntityUri) {
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
    fn compute_dfs_position(&self, id: &EntityUri, parent_id: &Option<EntityUri>) -> usize {
        let siblings = match self.children.get(parent_id) {
            Some(s) => s,
            None => return self.flat_order.len(),
        };

        // Find the sibling that comes right after us in sort order.
        let mut found_self = false;
        for sibling in siblings.iter() {
            if &sibling.id == id {
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
        let mut prev_sibling_id: Option<&EntityUri> = None;
        for sibling in siblings.iter() {
            if &sibling.id == id {
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
    fn update_has_children(&self, id: &EntityUri) {
        let Some(pos) = self.pos_of(id) else {
            return;
        };
        let node = &self.nodes[id];
        let has_children = self
            .children
            .get(&Some(id.clone()))
            .map_or(false, |c| !c.is_empty());
        let wrapped = wrap_tree_item(&node.widget, node.depth, has_children, &node.overrides);
        self.flat.lock_mut().set_cloned(pos, Arc::new(wrapped));
    }
}

// TODO: The ReactiveViewModel wrapping seems to be a separate concern. Move to a different/new file?
/// Wrap a widget in a TreeItem with the given depth and has_children flag.
/// Rule overrides are merged last, mirroring the static path's
/// `flat_tree_items` (shadow_builders/tree.rs).
fn wrap_tree_item(
    widget: &Arc<ReactiveViewModel>,
    depth: usize,
    has_children: bool,
    overrides: &HashMap<String, Value>,
) -> ReactiveViewModel {
    let mut props = std::collections::HashMap::new();
    props.insert("depth".to_string(), Value::Integer(depth as i64));
    props.insert("has_children".to_string(), Value::Boolean(has_children));
    for (k, v) in overrides {
        props.insert(k.clone(), v.clone());
    }
    ReactiveViewModel {
        children: vec![widget.clone()],
        data: futures_signals::signal::Mutable::new(widget.entity()).read_only(),
        // Per-instance collapse state. `true = expanded` (default — children
        // visible). Two `tree_item`s wrapping the same widget id get
        // independent `Mutable`s, so collapsing one doesn't collapse the
        // other. Preserved across structural rebuilds via
        // `with_update`'s `expanded: self.expanded.clone()` chain — the
        // `Mutable` handle is itself an `Arc` so the cell survives even
        // when the wrapping `ReactiveViewModel` is replaced.
        expanded: Some(futures_signals::signal::Mutable::new(true)),
        ..ReactiveViewModel::from_widget("tree_item", props)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive_view_model::ReactiveViewModel;

    fn widget(name: &str) -> Arc<ReactiveViewModel> {
        Arc::new(ReactiveViewModel::text(name))
    }

    /// Test-helper: a bare id becomes a `block:` EntityUri (the canonical
    /// key the matview pipeline produces).
    fn eu(s: &str) -> EntityUri {
        EntityUri::block(s)
    }

    fn make_tree() -> (MutableTree, MutableVec<Arc<ReactiveViewModel>>) {
        let flat = MutableVec::new();
        let tree = MutableTree::new(flat.clone());
        (tree, flat)
    }

    #[test]
    fn insert_root_nodes() {
        let (mut tree, flat) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(eu("b"), None, "1.0".into(), widget("B"), HashMap::new());

        assert_eq!(tree.flat_ids(), vec![eu("a"), eu("b")]);
        assert_eq!(flat.lock_ref().len(), 2);
    }

    #[test]
    fn insert_child_computes_depth() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("root"),
            None,
            "0.0".into(),
            widget("Root"),
            HashMap::new(),
        );
        tree.insert(
            eu("child"),
            Some(eu("root")),
            "0.0".into(),
            widget("Child"),
            HashMap::new(),
        );

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], (eu("root"), 0, true));
        assert_eq!(snap[1], (eu("child"), 1, false));
    }

    #[test]
    fn insert_grandchild() {
        let (mut tree, _) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(
            eu("b"),
            Some(eu("a")),
            "0.0".into(),
            widget("B"),
            HashMap::new(),
        );
        tree.insert(
            eu("c"),
            Some(eu("b")),
            "0.0".into(),
            widget("C"),
            HashMap::new(),
        );

        let snap = tree.flat_snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].1, 0); // a depth=0
        assert_eq!(snap[1].1, 1); // b depth=1
        assert_eq!(snap[2].1, 2); // c depth=2
    }

    /// A child whose parent has not been inserted yet must be re-parented when
    /// the parent finally arrives — not stranded as a permanent root.
    ///
    /// The reactive row set keys rows by `EntityUri`, so the collection driver
    /// feeds them in block-id order, which is NOT topological: a parent whose
    /// id sorts after its children (e.g. a freshly minted UUID doc-root above
    /// `bulk-*` blocks) arrives last. Stranding those early children as roots
    /// scrambled the rendered sibling order (they sorted among the roots
    /// instead of under their real parent).
    #[test]
    fn child_before_parent_is_reparented() {
        let (mut tree, _) = make_tree();
        // Children arrive first (parent "p" not yet present → would be roots).
        tree.insert(
            eu("c1"),
            Some(eu("p")),
            "10".into(),
            widget("C1"),
            HashMap::new(),
        );
        tree.insert(
            eu("c2"),
            Some(eu("p")),
            "20".into(),
            widget("C2"),
            HashMap::new(),
        );
        // Parent arrives last.
        tree.insert(eu("p"), None, "00".into(), widget("P"), HashMap::new());

        let snap = tree.flat_snapshot();
        assert_eq!(
            tree.flat_ids(),
            vec![eu("p"), eu("c1"), eu("c2")],
            "children must be re-parented under p in sort_key order"
        );
        assert_eq!(snap[0], (eu("p"), 0, true)); // p is a parent now
        assert_eq!(snap[1].1, 1); // c1 depth=1
        assert_eq!(snap[2].1, 1); // c2 depth=1
    }

    /// Re-parenting must be transitive: a grandchild stranded as a root gets
    /// pulled under its parent once that parent is itself re-parented.
    #[test]
    fn transitive_reparent_on_late_ancestor() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("gc"),
            Some(eu("c")),
            "0".into(),
            widget("GC"),
            HashMap::new(),
        );
        tree.insert(
            eu("c"),
            Some(eu("p")),
            "0".into(),
            widget("C"),
            HashMap::new(),
        );
        tree.insert(eu("p"), None, "0".into(), widget("P"), HashMap::new());

        let snap = tree.flat_snapshot();
        assert_eq!(tree.flat_ids(), vec![eu("p"), eu("c"), eu("gc")]);
        assert_eq!(snap[0].1, 0);
        assert_eq!(snap[1].1, 1);
        assert_eq!(snap[2].1, 2);
    }

    #[test]
    fn siblings_sorted_by_sort_key() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("root"),
            None,
            "0.0".into(),
            widget("Root"),
            HashMap::new(),
        );
        tree.insert(
            eu("c"),
            Some(eu("root")),
            "2.0".into(),
            widget("C"),
            HashMap::new(),
        );
        tree.insert(
            eu("a"),
            Some(eu("root")),
            "0.0".into(),
            widget("A"),
            HashMap::new(),
        );
        tree.insert(
            eu("b"),
            Some(eu("root")),
            "1.0".into(),
            widget("B"),
            HashMap::new(),
        );

        assert_eq!(tree.flat_ids(), vec![eu("root"), eu("a"), eu("b"), eu("c")]);
    }

    #[test]
    fn insert_between_siblings_with_children() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("root"),
            None,
            "0.0".into(),
            widget("Root"),
            HashMap::new(),
        );
        tree.insert(
            eu("s1"),
            Some(eu("root")),
            "0.0".into(),
            widget("S1"),
            HashMap::new(),
        );
        tree.insert(
            eu("s1c"),
            Some(eu("s1")),
            "0.0".into(),
            widget("S1-child"),
            HashMap::new(),
        );
        tree.insert(
            eu("s3"),
            Some(eu("root")),
            "2.0".into(),
            widget("S3"),
            HashMap::new(),
        );
        // Insert s2 between s1 and s3
        tree.insert(
            eu("s2"),
            Some(eu("root")),
            "1.0".into(),
            widget("S2"),
            HashMap::new(),
        );

        assert_eq!(
            tree.flat_ids(),
            vec![eu("root"), eu("s1"), eu("s1c"), eu("s2"), eu("s3")]
        );
    }

    #[test]
    fn update_data_only() {
        let (mut tree, flat) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("old"), HashMap::new());

        tree.update(&eu("a"), None, "0.0".into(), widget("new"), HashMap::new());

        assert_eq!(tree.flat_ids(), vec![eu("a")]);
        assert_eq!(flat.lock_ref().len(), 1);
    }

    #[test]
    fn update_reparent() {
        let (mut tree, _) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(eu("b"), None, "1.0".into(), widget("B"), HashMap::new());
        tree.insert(
            eu("c"),
            Some(eu("a")),
            "0.0".into(),
            widget("C"),
            HashMap::new(),
        );

        // Move c from under a to under b
        tree.update(
            &eu("c"),
            Some(eu("b")),
            "0.0".into(),
            widget("C"),
            HashMap::new(),
        );

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], (eu("a"), 0, false)); // a lost its child
        assert_eq!(snap[1], (eu("b"), 0, true)); // b gained a child
        assert_eq!(snap[2], (eu("c"), 1, false)); // c under b
    }

    /// Moving a node that has children must carry its whole subtree along —
    /// descendants only get an UpdateAt when their OWN row changes, so the
    /// tree can't rely on them being re-delivered.
    #[test]
    fn update_reparent_moves_subtree() {
        let (mut tree, flat) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(eu("b"), None, "1.0".into(), widget("B"), HashMap::new());
        tree.insert(
            eu("c"),
            Some(eu("a")),
            "0.0".into(),
            widget("C"),
            HashMap::new(),
        );
        tree.insert(
            eu("gc"),
            Some(eu("c")),
            "0.0".into(),
            widget("GC"),
            HashMap::new(),
        );

        // Indent c (with its child gc) from under a to under b.
        tree.update(
            &eu("c"),
            Some(eu("b")),
            "0.0".into(),
            widget("C"),
            HashMap::new(),
        );

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], (eu("a"), 0, false));
        assert_eq!(snap[1], (eu("b"), 0, true));
        assert_eq!(snap[2], (eu("c"), 1, true));
        assert_eq!(snap[3], (eu("gc"), 2, false));
        assert_eq!(flat.lock_ref().len(), 4);
    }

    /// Reordering (sort_key change only) must also keep the subtree intact.
    #[test]
    fn update_sort_key_moves_subtree() {
        let (mut tree, _) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(
            eu("ac"),
            Some(eu("a")),
            "0.0".into(),
            widget("AC"),
            HashMap::new(),
        );
        tree.insert(eu("b"), None, "1.0".into(), widget("B"), HashMap::new());

        // Move a (with child ac) after b.
        tree.update(&eu("a"), None, "2.0".into(), widget("A"), HashMap::new());

        assert_eq!(tree.flat_ids(), vec![eu("b"), eu("a"), eu("ac")]);
    }

    /// A transient parent-inside-own-subtree update must not lose the node
    /// (rendered as root until convergence), and must not cycle.
    #[test]
    fn update_parent_inside_own_subtree_renders_as_root() {
        let (mut tree, _) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(
            eu("b"),
            Some(eu("a")),
            "0.0".into(),
            widget("B"),
            HashMap::new(),
        );

        // Concurrent-move transient: a claims b (its own child) as parent.
        tree.update(
            &eu("a"),
            Some(eu("b")),
            "0.0".into(),
            widget("A"),
            HashMap::new(),
        );

        assert_eq!(tree.flat_ids(), vec![eu("a"), eu("b")]);
    }

    #[test]
    fn remove_leaf() {
        let (mut tree, flat) = make_tree();
        tree.insert(eu("a"), None, "0.0".into(), widget("A"), HashMap::new());
        tree.insert(eu("b"), None, "1.0".into(), widget("B"), HashMap::new());

        tree.remove(&eu("a"));

        assert_eq!(tree.flat_ids(), vec![eu("b")]);
        assert_eq!(flat.lock_ref().len(), 1);
    }

    #[test]
    fn remove_subtree() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("root"),
            None,
            "0.0".into(),
            widget("Root"),
            HashMap::new(),
        );
        tree.insert(
            eu("child"),
            Some(eu("root")),
            "0.0".into(),
            widget("Child"),
            HashMap::new(),
        );
        tree.insert(
            eu("grandchild"),
            Some(eu("child")),
            "0.0".into(),
            widget("GC"),
            HashMap::new(),
        );
        tree.insert(
            eu("other"),
            None,
            "1.0".into(),
            widget("Other"),
            HashMap::new(),
        );

        tree.remove(&eu("root"));

        assert_eq!(tree.flat_ids(), vec![eu("other")]);
    }

    #[test]
    fn remove_updates_parent_has_children() {
        let (mut tree, _) = make_tree();
        tree.insert(
            eu("parent"),
            None,
            "0.0".into(),
            widget("Parent"),
            HashMap::new(),
        );
        tree.insert(
            eu("child"),
            Some(eu("parent")),
            "0.0".into(),
            widget("Child"),
            HashMap::new(),
        );

        assert!(tree.flat_snapshot()[0].2); // has_children = true

        tree.remove(&eu("child"));

        assert!(!tree.flat_snapshot()[0].2); // has_children = false
    }

    #[test]
    fn rebuild_from_scratch() {
        let (mut tree, flat) = make_tree();
        tree.insert(eu("old"), None, "0.0".into(), widget("Old"), HashMap::new());

        tree.rebuild(vec![
            (eu("a"), None, "0.0".into(), widget("A"), HashMap::new()),
            (
                eu("b"),
                Some(eu("a")),
                "0.0".into(),
                widget("B"),
                HashMap::new(),
            ),
            (eu("c"), None, "1.0".into(), widget("C"), HashMap::new()),
        ]);

        assert_eq!(tree.flat_ids(), vec![eu("a"), eu("b"), eu("c")]);
        assert_eq!(flat.lock_ref().len(), 3);

        let snap = tree.flat_snapshot();
        assert_eq!(snap[0], (eu("a"), 0, true));
        assert_eq!(snap[1], (eu("b"), 1, false));
        assert_eq!(snap[2], (eu("c"), 0, false));
    }

    #[test]
    fn rebuild_ignores_missing_parents() {
        let (mut tree, _) = make_tree();
        tree.rebuild(vec![
            (
                eu("a"),
                Some(eu("nonexistent")),
                "0.0".into(),
                widget("A"),
                HashMap::new(),
            ),
            (
                eu("b"),
                Some(eu("a")),
                "0.0".into(),
                widget("B"),
                HashMap::new(),
            ),
        ]);

        let snap = tree.flat_snapshot();
        // "a" becomes a root because its parent doesn't exist in the dataset
        assert_eq!(snap[0], (eu("a"), 0, true));
        assert_eq!(snap[1], (eu("b"), 1, false));
    }
}
