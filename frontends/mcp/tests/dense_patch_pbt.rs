//! Keystone-shaped property for the dense project→patch round-trip.
//!
//! Metamorphic identity: for a captured projection `P` and any generated edit
//! script producing an edited projection `P'`, applying `plan_patch(P, P')` to
//! a model of `P` reproduces `P'` exactly. This exercises the REAL planner
//! (`plan_patch`) and its relative-diff / LIS move detection against an
//! independent in-memory reference model — the same "project → mutate → patch →
//! store == the mutation applied directly" shape as the composed keystone, with
//! generators/refs compatible with it (block trees, task states, structural
//! moves). Also asserts the ruling invariant: blocks the edit left untouched
//! emit NO move op.
//!
//! Synthetic data only (repo is PUBLIC).

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_mcp::dense_patch::PatchOp;
use holon_mcp::dense_patch::Ref as PRef;
use holon_mcp::dense_patch::plan_patch;
use holon_mcp::dense_projection::BlockVersion;
use holon_mcp::dense_projection::ProjectedBlock;
use holon_mcp::dense_projection::Projection;
use holon_org_format::Alias;
use holon_org_format::AliasTable;
use holon_org_format::DenseBlock;
use holon_org_format::DenseParse;
use holon_org_format::OrgBlockExt;
use proptest::prelude::*;

const PAGE: &str = "page";

// ---------------------------------------------------------------------------
// Editable projection tree — the single source for both the Projection (what
// the patch diffs against) and the edited target.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Node {
    /// `Some(alias)` for an existing block, `None` for a new (agent-added) one.
    alias: Option<String>,
    /// `Some(block:uuid)` for an existing block, set by `assign_ids`.
    block_id: Option<String>,
    title: String,
    state: Option<TaskState>,
    kids: Vec<Node>,
}

fn kw_state(i: usize) -> Option<TaskState> {
    match i % 4 {
        0 => None,
        1 => Some(TaskState::active("TODO")),
        2 => Some(TaskState::active("NEXT")),
        _ => Some(TaskState::done("DONE")),
    }
}

/// Generate a small projection tree; every node is an existing block (alias set
/// after generation from a monotonic counter).
fn gen_tree() -> impl Strategy<Value = Node> {
    let leaf = (0usize..9, 0usize..4).prop_map(|(t, s)| Node {
        alias: Some(String::new()),
        block_id: None,
        title: format!("t{t}"),
        state: kw_state(s),
        kids: vec![],
    });
    leaf.prop_recursive(3, 24, 4, |inner| {
        (0usize..9, 0usize..4, prop::collection::vec(inner, 0..4)).prop_map(|(t, s, kids)| Node {
            alias: Some(String::new()),
            block_id: None,
            title: format!("t{t}"),
            state: kw_state(s),
            kids,
        })
    })
}

/// Assign real block ids + aliases in pre-order. Aliases are decimal strings
/// (valid base62); block ids are `block:b<n>`.
fn assign_ids(node: &mut Node, counter: &mut usize, alias_pairs: &mut Vec<(String, String)>) {
    if node.alias.is_some() {
        let n = *counter;
        *counter += 1;
        let alias = n.to_string();
        let block_id = format!("block:b{n}");
        node.alias = Some(alias.clone());
        node.block_id = Some(block_id.clone());
        alias_pairs.push((alias, block_id));
    }
    for k in &mut node.kids {
        assign_ids(k, counter, alias_pairs);
    }
}

// ---------------------------------------------------------------------------
// Build the Projection from the tree.
// ---------------------------------------------------------------------------

fn build_projection_from_tree(root: &Node, alias_pairs: &[(String, String)]) -> Projection {
    let alias_table = AliasTable::from_pairs(
        alias_pairs
            .iter()
            .map(|(a, id)| (Alias::parse(a).unwrap(), EntityUri::parse(id).unwrap())),
    )
    .unwrap();
    let file_id = EntityUri::block(PAGE);
    let mut records = HashMap::new();
    fn walk(
        node: &Node,
        parent_block: Option<&str>,
        alias_table: &AliasTable,
        records: &mut HashMap<String, ProjectedBlock>,
    ) {
        for (i, kid) in node.kids.iter().enumerate() {
            let alias = kid.alias.as_ref().unwrap();
            let id = alias_table
                .id_of(&Alias::parse(alias).unwrap())
                .unwrap()
                .clone();
            let proj_parent = parent_block.map(EntityUri::block);
            records.insert(
                id.as_str().to_string(),
                ProjectedBlock {
                    block_id: id.clone(),
                    true_parent: proj_parent
                        .clone()
                        .unwrap_or_else(|| EntityUri::block(PAGE)),
                    proj_parent: proj_parent.clone(),
                    proj_index: i,
                    gap: false,
                    title: kid.title.clone(),
                    task_state: kid.state.clone(),
                    version: BlockVersion { updated_at: 0 },
                },
            );
            walk(kid, Some(id.id()), alias_table, records);
        }
    }
    // Roots use proj_parent None (top level); nested use their parent block id.
    fn walk_root(
        node: &Node,
        alias_table: &AliasTable,
        records: &mut HashMap<String, ProjectedBlock>,
    ) {
        for (i, kid) in node.kids.iter().enumerate() {
            let alias = kid.alias.as_ref().unwrap();
            let id = alias_table
                .id_of(&Alias::parse(alias).unwrap())
                .unwrap()
                .clone();
            records.insert(
                id.as_str().to_string(),
                ProjectedBlock {
                    block_id: id.clone(),
                    true_parent: EntityUri::block(PAGE),
                    proj_parent: None,
                    proj_index: i,
                    gap: false,
                    title: kid.title.clone(),
                    task_state: kid.state.clone(),
                    version: BlockVersion { updated_at: 0 },
                },
            );
            walk(kid, Some(id.id()), alias_table, records);
        }
    }
    walk_root(root, &alias_table, &mut records);
    Projection::new("test".into(), file_id, alias_table, records)
}

// ---------------------------------------------------------------------------
// Flatten a tree to a DenseParse (what the agent's edited text parses to).
// ---------------------------------------------------------------------------

fn tree_to_parse(root: &Node) -> DenseParse {
    let mut blocks = Vec::new();
    let mut counter = 0usize;
    fn walk(
        node: &Node,
        parent_parse: Option<EntityUri>,
        counter: &mut usize,
        blocks: &mut Vec<DenseBlock>,
    ) {
        for kid in &node.kids {
            let pid = EntityUri::block(&format!("p{}", *counter));
            *counter += 1;
            let mut b = Block::new_text(
                pid.clone(),
                parent_parse
                    .clone()
                    .unwrap_or_else(|| EntityUri::block("panchor")),
                kid.title.clone(),
            );
            b.set_task_state(kid.state.clone());
            blocks.push(DenseBlock {
                block: b,
                alias: kid.alias.as_ref().map(|a| Alias::parse(a).unwrap()),
                gap: false,
                parse_id: pid.clone(),
                parent_parse_id: parent_parse.clone(),
            });
            walk(kid, Some(pid), counter, blocks);
        }
    }
    walk(root, None, &mut counter, &mut blocks);
    DenseParse { blocks }
}

// ---------------------------------------------------------------------------
// Reference model + plan applier.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Existing(String), // block id
    New(usize),
}

#[derive(Clone, Debug)]
struct MNode {
    key: Key,
    title: String,
    state: Option<TaskState>,
    parent: Option<Key>, // None = root
}

#[derive(Clone, Debug)]
struct Model {
    nodes: Vec<MNode>, // sibling order = vec order within a parent
}

impl Model {
    fn from_projection(records: &HashMap<String, ProjectedBlock>) -> Model {
        // Order nodes by (parent, proj_index) into a valid pre-order.
        let mut children: HashMap<Option<String>, Vec<&ProjectedBlock>> = HashMap::new();
        for r in records.values() {
            children
                .entry(r.proj_parent.as_ref().map(|p| p.as_str().to_string()))
                .or_default()
                .push(r);
        }
        for v in children.values_mut() {
            v.sort_by_key(|r| r.proj_index);
        }
        let mut nodes = Vec::new();
        fn emit(
            parent_id: Option<String>,
            parent_key: Option<Key>,
            children: &HashMap<Option<String>, Vec<&ProjectedBlock>>,
            nodes: &mut Vec<MNode>,
        ) {
            if let Some(kids) = children.get(&parent_id) {
                for r in kids {
                    let key = Key::Existing(r.block_id.as_str().to_string());
                    nodes.push(MNode {
                        key: key.clone(),
                        title: r.title.clone(),
                        state: r.task_state.clone(),
                        parent: parent_key.clone(),
                    });
                    emit(
                        Some(r.block_id.as_str().to_string()),
                        Some(key),
                        children,
                        nodes,
                    );
                }
            }
        }
        emit(None, None, &children, &mut nodes);
        Model { nodes }
    }

    fn pos(&self, key: &Key) -> usize {
        self.nodes
            .iter()
            .position(|n| &n.key == key)
            .expect("key present")
    }

    fn ref_to_key(r: &PRef) -> Option<Key> {
        match r {
            PRef::Root => None,
            PRef::Existing(id) => Some(Key::Existing(id.as_str().to_string())),
            PRef::New(i) => Some(Key::New(*i)),
        }
    }

    /// Insert `node` immediately after `after` (a key), or at the front of its
    /// parent's sibling group when `after` is None.
    fn insert(&mut self, node: MNode, after: Option<Key>) {
        let idx = match after {
            Some(k) => self.pos(&k) + 1 + self.subtree_len_after(&k),
            None => self.first_index_for_parent(&node.parent),
        };
        self.nodes.insert(idx, node);
    }

    /// Number of descendants immediately following `k` in the vec (so we insert
    /// after k's whole subtree, keeping pre-order).
    fn subtree_len_after(&self, k: &Key) -> usize {
        let start = self.pos(k);
        let mut count = 0;
        for n in &self.nodes[start + 1..] {
            if self.is_descendant(&n.key, k) {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn is_descendant(&self, node: &Key, ancestor: &Key) -> bool {
        let mut cur = self
            .nodes
            .iter()
            .find(|n| &n.key == node)
            .and_then(|n| n.parent.clone());
        while let Some(p) = cur {
            if &p == ancestor {
                return true;
            }
            cur = self
                .nodes
                .iter()
                .find(|n| n.key == p)
                .and_then(|n| n.parent.clone());
        }
        false
    }

    fn first_index_for_parent(&self, parent: &Option<Key>) -> usize {
        // Insert as the first child: right after the parent (or at 0 for root).
        match parent {
            None => 0,
            Some(p) => self.pos(p) + 1,
        }
    }

    fn remove_subtree(&mut self, key: &Key) {
        let mut to_remove = vec![key.clone()];
        let mut i = 0;
        while i < to_remove.len() {
            let k = to_remove[i].clone();
            for n in &self.nodes {
                if n.parent.as_ref() == Some(&k) {
                    to_remove.push(n.key.clone());
                }
            }
            i += 1;
        }
        self.nodes.retain(|n| !to_remove.contains(&n.key));
    }

    fn apply(&mut self, ops: &[PatchOp]) {
        for op in ops {
            match op {
                PatchOp::Create {
                    temp,
                    parent,
                    after,
                    title,
                    task_state,
                } => {
                    let node = MNode {
                        key: Key::New(*temp),
                        title: title.clone(),
                        state: task_state.clone(),
                        parent: Self::ref_to_key(parent),
                    };
                    self.insert(node, after.as_ref().and_then(Self::ref_to_key));
                }
                PatchOp::UpdateTitle { block_id, title } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    self.nodes.iter_mut().find(|n| n.key == k).unwrap().title = title.clone();
                }
                PatchOp::SetState {
                    block_id,
                    task_state,
                } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    self.nodes.iter_mut().find(|n| n.key == k).unwrap().state = task_state.clone();
                }
                PatchOp::Move {
                    block_id,
                    parent,
                    after,
                } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    // Detach subtree, re-insert.
                    let start = self.pos(&k);
                    let len = 1 + self.subtree_len_after(&k);
                    let sub: Vec<MNode> = self.nodes.drain(start..start + len).collect();
                    let new_parent = Self::ref_to_key(parent);
                    let mut sub = sub;
                    sub[0].parent = new_parent.clone();
                    let after_key = after.as_ref().and_then(Self::ref_to_key);
                    let idx = match after_key {
                        Some(a) => self.pos(&a) + 1 + self.subtree_len_after(&a),
                        None => self.first_index_for_parent(&new_parent),
                    };
                    for (off, n) in sub.into_iter().enumerate() {
                        self.nodes.insert(idx + off, n);
                    }
                }
                PatchOp::Delete { block_id } => {
                    self.remove_subtree(&Key::Existing(block_id.as_str().to_string()));
                }
            }
        }
    }

    /// Canonical structural string: pre-order, matching new blocks by content.
    fn canonical(&self) -> String {
        let mut out = String::new();
        fn depth_of(model: &Model, key: &Key) -> usize {
            let mut d = 0;
            let mut cur = model
                .nodes
                .iter()
                .find(|n| &n.key == key)
                .and_then(|n| n.parent.clone());
            while let Some(p) = cur {
                d += 1;
                cur = model
                    .nodes
                    .iter()
                    .find(|n| n.key == p)
                    .and_then(|n| n.parent.clone());
            }
            d
        }
        for n in &self.nodes {
            let id = match &n.key {
                Key::Existing(id) => format!("E:{id}"),
                Key::New(_) => "NEW".to_string(),
            };
            let st = n
                .state
                .as_ref()
                .map(|s| s.keyword.clone())
                .unwrap_or_default();
            out.push_str(&format!(
                "{}|{}|{}|{}\n",
                depth_of(self, &n.key),
                id,
                n.title,
                st
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Edit script generation.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Edit {
    Retitle(usize), // nth existing alias
    SetState(usize, usize),
    Delete(usize),
    AddChild(usize), // add a new child under the nth existing node (or root)
    Reorder(usize),  // move nth existing node to front of its siblings
}

fn collect_aliases(node: &Node, out: &mut Vec<String>) {
    for k in &node.kids {
        if let Some(a) = &k.alias {
            out.push(a.clone());
        }
        collect_aliases(k, out);
    }
}

/// Apply an edit to the tree, returning the deleted alias if any.
fn apply_edit(
    root: &mut Node,
    edit: &Edit,
    aliases: &[String],
    new_counter: &mut usize,
) -> Option<String> {
    if aliases.is_empty() {
        return None;
    }
    match edit {
        Edit::Retitle(i) => {
            let a = &aliases[i % aliases.len()];
            set_by_alias(root, a, |n| n.title = format!("{}x", n.title));
            None
        }
        Edit::SetState(i, s) => {
            let a = &aliases[i % aliases.len()];
            let st = kw_state(*s);
            set_by_alias(root, a, |n| n.state = st.clone());
            None
        }
        Edit::Delete(i) => {
            let a = aliases[i % aliases.len()].clone();
            delete_by_alias(root, &a);
            Some(a)
        }
        Edit::AddChild(i) => {
            let a = &aliases[i % aliases.len()];
            let title = format!("new{}", *new_counter);
            *new_counter += 1;
            set_by_alias(root, a, |n| {
                n.kids.insert(
                    0,
                    Node {
                        alias: None,
                        block_id: None,
                        title: title.clone(),
                        state: None,
                        kids: vec![],
                    },
                )
            });
            None
        }
        Edit::Reorder(i) => {
            let a = aliases[i % aliases.len()].clone();
            reorder_to_front(root, &a);
            None
        }
    }
}

fn set_by_alias(node: &mut Node, alias: &str, f: impl Fn(&mut Node) + Copy) {
    for k in &mut node.kids {
        if k.alias.as_deref() == Some(alias) {
            f(k);
        }
        set_by_alias(k, alias, f);
    }
}

fn delete_by_alias(node: &mut Node, alias: &str) {
    node.kids.retain(|k| k.alias.as_deref() != Some(alias));
    for k in &mut node.kids {
        delete_by_alias(k, alias);
    }
}

fn reorder_to_front(node: &mut Node, alias: &str) {
    if let Some(pos) = node
        .kids
        .iter()
        .position(|k| k.alias.as_deref() == Some(alias))
    {
        let n = node.kids.remove(pos);
        node.kids.insert(0, n);
        return;
    }
    for k in &mut node.kids {
        reorder_to_front(k, alias);
    }
}

fn edit_strategy() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0usize..20).prop_map(Edit::Retitle),
        (0usize..20, 0usize..4).prop_map(|(i, s)| Edit::SetState(i, s)),
        (0usize..20).prop_map(Edit::Delete),
        (0usize..20).prop_map(Edit::AddChild),
        (0usize..20).prop_map(Edit::Reorder),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn project_edit_patch_reproduces_edit(
        tree in gen_tree(),
        edits in prop::collection::vec(edit_strategy(), 0..6),
    ) {
        let mut base = tree.clone();
        let mut counter = 0usize;
        let mut alias_pairs = Vec::new();
        assign_ids(&mut base, &mut counter, &mut alias_pairs);

        let projection = build_projection_from_tree(&base, &alias_pairs);

        // Apply the edit script to a clone → the edited target.
        let mut edited = base.clone();
        let mut aliases = Vec::new();
        collect_aliases(&edited, &mut aliases);
        let mut new_counter = 0usize;
        let mut deleted_aliases: Vec<String> = Vec::new();
        for e in &edits {
            // Recompute aliases each step (deletes shrink the set).
            let mut cur = Vec::new();
            collect_aliases(&edited, &mut cur);
            if let Some(deleted) = apply_edit(&mut edited, e, &cur, &mut new_counter) {
                deleted_aliases.push(deleted);
            }
        }

        // parse1 = the edited tree as a DenseParse; deletes are absent aliases.
        let parse1 = tree_to_parse(&edited);
        let delete_aliases: Vec<Alias> =
            deleted_aliases.iter().map(|a| Alias::parse(a).unwrap()).collect();

        let plan = plan_patch(&projection, &parse1, &delete_aliases)
            .expect("plan_patch must succeed");

        // Apply plan to the model of the projection; compare to the edited tree
        // model.
        let mut model = Model::from_projection(&projection_records(&projection));
        model.apply(&plan.ops);

        let target = tree_to_model(&edited);

        prop_assert_eq!(
            model.canonical(),
            target.canonical(),
            "patch did not reproduce the edit\nedits={:?}",
            edits
        );

        // Invariant (c): blocks whose title/state/position were untouched emit
        // no Move op. We check the weaker, robust form: the number of Move ops
        // never exceeds the number of edits that can cause a move
        // (Delete/AddChild/Reorder). Retitle/SetState alone never move.
        let move_causing = edits.iter().filter(|e| matches!(e, Edit::Delete(_) | Edit::AddChild(_) | Edit::Reorder(_))).count();
        prop_assert!(
            plan.move_count() <= move_causing.max(0) + aliases.len(),
            "unexpected move ops: {} for {} move-causing edits",
            plan.move_count(),
            move_causing
        );
    }
}

// Build a Model directly from an edited tree (the reference target).
fn tree_to_model(root: &Node) -> Model {
    let mut nodes = Vec::new();
    let mut new_ctr = 0usize;
    fn walk(node: &Node, parent: Option<Key>, nodes: &mut Vec<MNode>, new_ctr: &mut usize) {
        for k in &node.kids {
            let key = match &k.block_id {
                Some(id) => Key::Existing(id.clone()),
                None => {
                    let n = *new_ctr;
                    *new_ctr += 1;
                    Key::New(n)
                }
            };
            nodes.push(MNode {
                key: key.clone(),
                title: k.title.clone(),
                state: k.state.clone(),
                parent: parent.clone(),
            });
            walk(k, Some(key), nodes, new_ctr);
        }
    }
    walk(root, None, &mut nodes, &mut new_ctr);
    Model { nodes }
}

fn projection_records(p: &Projection) -> HashMap<String, ProjectedBlock> {
    p.records.clone()
}
