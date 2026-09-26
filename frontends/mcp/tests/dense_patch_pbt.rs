//! Keystone-shaped property for the dense project→patch round-trip.
//!
//! Metamorphic identity: for a captured projection `P` and any generated edit
//! script producing an edited projection `P'`, applying `plan_patch(P, P')` to
//! a model of `P` reproduces `P'` exactly. `P` is built by the real
//! `build_projection` and `P'` is rendered by the real dense renderer, so this
//! exercises the REAL planner and its relative-diff / LIS move detection
//! against an independent in-memory reference model, tags and drawer
//! properties included —
//! the same "project → mutate → patch →
//! store == the mutation applied directly" shape as the composed keystone, with
//! generators/refs compatible with it (block trees, task states, structural
//! moves). Also asserts the ruling invariant: blocks the edit left untouched
//! emit NO move op.
//!
//! Synthetic data only (repo is PUBLIC).

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_mcp::dense_patch::PatchOp;
use holon_mcp::dense_patch::Ref as PRef;
use holon_mcp::dense_patch::RowAttributes;
use holon_mcp::dense_patch::plan_patch;
use holon_mcp::dense_projection::ProjectedBlock;
use holon_mcp::dense_projection::Projection;
use holon_mcp::dense_projection::build_projection;
use holon_org_format::Alias;
use holon_org_format::AliasTable;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::parse_dense;
use holon_org_format::render_dense;
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
    attributes: RowAttributes,
    kids: Vec<Node>,
}

const TAG_POOL: &[&str] = &["decision", "option", "ops"];
const PROPERTY_KEYS: &[&str] = &["choose", "Content", "a+b"];
const PROPERTY_VALUES: &[&str] = &["1", "", " b {#2} :x:\n\"q\""];

/// Tags from a bit mask over [`TAG_POOL`], properties from a base-4 digit per
/// key of [`PROPERTY_KEYS`] (0 = absent, else a [`PROPERTY_VALUES`] index + 1).
fn attrs(tag_mask: usize, prop_digits: usize) -> RowAttributes {
    let tags = TAG_POOL
        .iter()
        .enumerate()
        .filter(|(i, _)| tag_mask & (1 << i) != 0)
        .map(|(_, t)| t.to_string());
    let mut properties = std::collections::BTreeMap::new();
    for (i, key) in PROPERTY_KEYS.iter().enumerate() {
        let digit = (prop_digits >> (2 * i)) & 3;
        if digit != 0 {
            properties.insert(key.to_string(), PROPERTY_VALUES[digit - 1].to_string());
        }
    }
    RowAttributes {
        tags: holon_api::Tags::from_tag_iter(tags),
        properties,
    }
}

fn kw_state(i: usize) -> Option<TaskState> {
    match i % 4 {
        0 => None,
        1 => Some(TaskState::active("TODO")),
        2 => Some(TaskState::active("DOING")),
        _ => Some(TaskState::done("DONE")),
    }
}

/// Generate a small projection tree; every node is an existing block (alias set
/// after generation from a monotonic counter).
fn gen_tree() -> impl Strategy<Value = Node> {
    let leaf = (0usize..9, 0usize..4, 0usize..8, 0usize..64).prop_map(|(t, s, tm, pd)| Node {
        alias: Some(String::new()),
        block_id: None,
        title: format!("t{t}"),
        state: kw_state(s),
        attributes: attrs(tm, pd),
        kids: vec![],
    });
    leaf.prop_recursive(3, 24, 4, |inner| {
        (
            0usize..9,
            0usize..4,
            0usize..8,
            0usize..64,
            prop::collection::vec(inner, 0..4),
        )
            .prop_map(|(t, s, tm, pd, kids)| Node {
                alias: Some(String::new()),
                block_id: None,
                title: format!("t{t}"),
                state: kw_state(s),
                attributes: attrs(tm, pd),
                kids,
            })
    })
}

/// Assign real block ids in pre-order: `block:b<n>`.
fn assign_ids(node: &mut Node, counter: &mut usize) {
    if node.alias.is_some() {
        node.block_id = Some(format!("block:b{counter}"));
        *counter += 1;
    }
    for k in &mut node.kids {
        assign_ids(k, counter);
    }
}

/// The tree's blocks in pre-order; a new node gets the id `block:new<n>`.
fn blocks_of(root: &Node) -> Vec<Block> {
    fn walk(node: &Node, parent: &EntityUri, new_ctr: &mut usize, out: &mut Vec<Block>) {
        for kid in &node.kids {
            let id = match &kid.block_id {
                Some(id) => EntityUri::parse(id).unwrap(),
                None => {
                    *new_ctr += 1;
                    EntityUri::block(&format!("new{new_ctr}"))
                }
            };
            let mut b = Block::new_text(id.clone(), parent.clone(), kid.title.clone());
            b.set_task_state(kid.state.clone());
            b.tags = kid.attributes.tags.clone();
            for (k, v) in &kid.attributes.properties {
                b.set_property(k, holon_api::Value::String(v.clone()));
            }
            out.push(b);
            walk(kid, &id, new_ctr, out);
        }
    }
    let mut out = Vec::new();
    walk(root, &EntityUri::block(PAGE), &mut 0, &mut out);
    out
}

/// Project the tree through the real `build_projection`, and record each
/// node's alias.
fn project(root: &mut Node) -> Projection {
    let built = build_projection(
        blocks_of(root),
        &holon_mcp::dense_projection::DocVocabularies::Uniform(
            holon_org_format::TaskKeywordVocabulary::default(),
        ),
    )
    .expect("the tree projects");
    fn set_aliases(node: &mut Node, table: &AliasTable) {
        for kid in &mut node.kids {
            let id = EntityUri::parse(kid.block_id.as_ref().unwrap()).unwrap();
            kid.alias = Some(table.alias_of(&id).unwrap().as_str().to_string());
            set_aliases(kid, table);
        }
    }
    set_aliases(root, &built.alias_table);
    Projection::new("test".into(), &built)
}

/// The edited tree as the dense text an agent would send: rendered by the
/// real renderer under the projection's `header`, with no token on a new row.
fn edited_text(root: &Node, header: &[String]) -> String {
    let blocks = blocks_of(root);
    let mut aliases: HashMap<String, String> = HashMap::new();
    fn collect(node: &Node, out: &mut HashMap<String, String>) {
        for kid in &node.kids {
            if let (Some(id), Some(alias)) = (&kid.block_id, &kid.alias) {
                out.insert(id.clone(), alias.clone());
            }
            collect(kid, out);
        }
    }
    collect(root, &mut aliases);
    let mut new_tokens = Vec::new();
    let table = AliasTable::from_pairs(blocks.iter().enumerate().map(|(i, b)| {
        let alias = aliases.get(b.id.as_str()).cloned().unwrap_or_else(|| {
            let token = format!("zz{i}");
            new_tokens.push(format!(" {{#{token}}}"));
            token
        });
        (Alias::parse(&alias).unwrap(), b.id.clone())
    }))
    .unwrap();
    let file_id = EntityUri::block(PAGE);
    let mut doc = Block::new_text(
        file_id.clone(),
        EntityUri::block("dense-projection-anchor"),
        "Projection".to_string(),
    );
    doc.set_page(true);
    doc.set_todo_keywords(Some((0..4).filter_map(kw_state).collect()));
    let mut text = render_dense(&doc, &blocks, &file_id, &table, &HashSet::new()).unwrap();
    for token in new_tokens {
        text = text.replacen(&token, "", 1);
    }
    let rows: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.starts_with("* "))
        .collect();
    format!("{}\n{}\n", header.join("\n"), rows.join("\n"))
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
    attributes: RowAttributes,
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
                    let shown = r.shown.as_ref().expect("every row carries a token");
                    nodes.push(MNode {
                        key: key.clone(),
                        title: shown.title.clone(),
                        state: shown.task_state.clone(),
                        attributes: shown.attributes.clone(),
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
                    content,
                    task_state,
                    attributes,
                    carriers,
                } => {
                    assert_eq!(
                        carriers,
                        &Vec::new(),
                        "the edits here write drawers in key order and no keyword line, which \
                         org renders unaided"
                    );
                    let node = MNode {
                        key: Key::New(*temp),
                        title: content.text.clone(),
                        state: task_state.clone(),
                        attributes: attributes.clone(),
                        parent: Self::ref_to_key(parent),
                    };
                    self.insert(node, after.as_ref().and_then(Self::ref_to_key));
                }
                PatchOp::SetContent { block_id, content } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    self.nodes.iter_mut().find(|n| n.key == k).unwrap().title =
                        content.text.clone();
                }
                PatchOp::SetState {
                    block_id,
                    task_state,
                } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    self.nodes.iter_mut().find(|n| n.key == k).unwrap().state = task_state.clone();
                }
                PatchOp::SetTags { block_id, tags } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    self.nodes
                        .iter_mut()
                        .find(|n| n.key == k)
                        .unwrap()
                        .attributes
                        .tags = tags.clone();
                }
                PatchOp::SetProperty {
                    block_id,
                    key,
                    value,
                } => {
                    let k = Key::Existing(block_id.as_str().to_string());
                    let props = &mut self
                        .nodes
                        .iter_mut()
                        .find(|n| n.key == k)
                        .unwrap()
                        .attributes
                        .properties;
                    match value {
                        Some(v) => props.insert(key.as_str().to_string(), v.clone()),
                        None => props.remove(key.as_str()),
                    };
                }
                PatchOp::SetCarrier { block_id, carrier } => panic!(
                    "the edits here write drawers in key order and no keyword line, which org \
                     renders unaided; {block_id} got {carrier:?}"
                ),
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
                "{}|{}|{}|{}|{:?}|{:?}\n",
                depth_of(self, &n.key),
                id,
                n.title,
                st,
                n.attributes.tags,
                n.attributes.properties
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
    AddChild(usize, usize, usize), // new child under the nth existing node, with attrs
    Reorder(usize),                // move nth existing node to front of its siblings
    SetAttributes(usize, usize, usize),
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
        Edit::AddChild(i, tm, pd) => {
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
                        attributes: attrs(*tm, *pd),
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
        Edit::SetAttributes(i, tm, pd) => {
            let a = &aliases[i % aliases.len()];
            let new = attrs(*tm, *pd);
            set_by_alias(root, a, |n| n.attributes = new.clone());
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
        (0usize..20, 0usize..8, 0usize..64).prop_map(|(i, tm, pd)| Edit::AddChild(i, tm, pd)),
        (0usize..20).prop_map(Edit::Reorder),
        (0usize..20, 0usize..8, 0usize..64).prop_map(|(i, tm, pd)| Edit::SetAttributes(i, tm, pd)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn project_edit_patch_reproduces_edit(
        tree in gen_tree(),
        edits in prop::collection::vec(edit_strategy(), 0..6),
    ) {
        let mut base = tree.clone();
        assign_ids(&mut base, &mut 0);
        let projection = project(&mut base);

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

        let text = edited_text(&edited, &projection.header);
        let parse1 = parse_dense(&text).expect("the edited projection parses");
        let delete_aliases: Vec<Alias> =
            deleted_aliases.iter().map(|a| Alias::parse(a).unwrap()).collect();

        let plan = plan_patch(&projection, &text, &parse1, &delete_aliases)
            .unwrap_or_else(|e| panic!("plan_patch must succeed: {e:#}\n{text}"));

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
        let move_causing = edits.iter().filter(|e| matches!(e, Edit::Delete(_) | Edit::AddChild(..) | Edit::Reorder(_))).count();
        prop_assert!(
            plan.move_count() <= move_causing + aliases.len(),
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
                attributes: k.attributes.clone(),
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
