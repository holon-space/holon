//! The three datom index trees, and the order they are kept in.
//!
//! A LogSeq graph holds its datoms three times over, in B+-trees keyed
//! `(e,a,v,tx)`, `(a,e,v,tx)` and `(a,v,e,tx)`. Increment B has to put a datom
//! where LogSeq would have put it, which is entirely a question of this
//! ordering — so the comparator here is the foundation the writer stands on.
//!
//! Every rule it implements was MEASURED against LogSeq's own runtime rather
//! than read out of datascript; the measurements, and the two rules that a
//! plausible reading gets wrong, are in docs/Testing/LogseqDbTreeOrder.md. The
//! two worth repeating here because they look wrong:
//!
//! - An attribute sorts by `(namespace, name)`, NOT by its printed form.
//! - Values of different types never interleave, and the group order is `bool <
//!   inst < number < string < uuid < other < keyword < list`.
//!
//! Stored datoms are always ASSERTIONS: the trees hold current state, and a
//! retraction exists only in the tail. Their `tx` is therefore positive, and
//! it is compared by magnitude in any case.

use std::cmp::Ordering;

use crate::TransitNode;
use crate::kvs_writer::KvsGraph;
use crate::kvs_writer::RowError;

/// One datom as an index tree stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeDatom {
    pub e: i64,
    /// The attribute ident without its leading colon.
    pub a: String,
    pub v: TransitNode,
    pub tx: i64,
}

/// Which index, and therefore which sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Eavt,
    Aevt,
    Avet,
}

impl Index {
    /// The root address this index hangs from.
    pub fn root_addr(self, graph: &KvsGraph) -> i64 {
        match self {
            Index::Eavt => graph.root.eavt,
            Index::Aevt => graph.root.aevt,
            Index::Avet => graph.root.avet,
        }
    }

    /// Order two datoms as this index does.
    ///
    /// Lexicographic over the index's components, short-circuiting on the
    /// first that differs — which is why a value comparison happens only once
    /// everything before it has tied.
    pub fn compare(self, a: &TreeDatom, b: &TreeDatom) -> Result<Ordering, RowError> {
        let e = || a.e.cmp(&b.e);
        let attr = || keyword_key(&a.a).cmp(&keyword_key(&b.a));
        // `datom-tx` is the MAGNITUDE: the sign carries assert-vs-retract, not
        // order. Stored datoms are all assertions, but comparing by magnitude
        // keeps this correct for a tail datom handed to it.
        let tx = || a.tx.abs().cmp(&b.tx.abs());

        let order = match self {
            Index::Eavt => [e(), attr(), Ordering::Equal, tx()],
            Index::Aevt => [attr(), e(), Ordering::Equal, tx()],
            Index::Avet => [attr(), Ordering::Equal, e(), tx()],
        };
        // The value slot is filled lazily: its comparison is the only one that
        // can fail, so it must not run unless it is actually reached.
        let value_slot = match self {
            Index::Eavt | Index::Aevt => 2,
            Index::Avet => 1,
        };
        for (i, ordering) in order.iter().enumerate() {
            if i == value_slot {
                let by_value = compare_values(&a.v, &b.v)?;
                if by_value != Ordering::Equal {
                    return Ok(by_value);
                }
            } else if *ordering != Ordering::Equal {
                return Ok(*ordering);
            }
        }
        Ok(Ordering::Equal)
    }
}

/// A keyword sorts by `(namespace, name)`, never by its printed form.
///
/// The difference is not cosmetic: `/` (47) sorts after `-` (45) and `.` (46),
/// so `:b/z` precedes `:b-a/a` by this rule and follows it by the other. Used
/// for attributes AND for keyword values, which measured the same.
fn keyword_key(ident: &str) -> (&str, &str) {
    match ident.split_once('/') {
        Some((namespace, name)) => (namespace, name),
        None => ("", ident),
    }
}

/// Which type group a value belongs to.
///
/// Values of different types never interleave, and this order was measured
/// pairwise through the index itself. It is NOT the order a reading of
/// datascript's `class-compare` suggests — see the doc.
fn type_rank(value: &TransitNode) -> u8 {
    match value {
        TransitNode::Bool(_) => 0,
        TransitNode::Instant(_) | TransitNode::InstantMillis(_) => 1,
        TransitNode::Int(_) | TransitNode::Float(_) => 2,
        TransitNode::Str(_) => 3,
        TransitNode::Uuid(_) => 4,
        // Neither a comparable native nor sequential: ClojureScript orders
        // these by `hash`, which is exactly what this build refuses to write.
        TransitNode::Nil
        | TransitNode::Symbol(_)
        | TransitNode::Map(_)
        | TransitNode::Tagged(..) => 5,
        TransitNode::Keyword(_) => 6,
        TransitNode::List(_) => 7,
    }
}

fn kind_of(value: &TransitNode) -> &'static str {
    match value {
        TransitNode::Map(_) => "map",
        TransitNode::Tagged(..) => "tagged",
        TransitNode::Symbol(_) => "symbol",
        TransitNode::Nil => "nil",
        _ => "value",
    }
}

/// Order two datom values.
///
/// Different type groups are decided by the group alone — which is why an
/// existing map-valued datom can be compared against anything Holon writes
/// without a hash. Two values in the hash-ordered group is the one case that
/// cannot be decided, and it is unreachable unless Holon wrote one.
fn compare_values(a: &TransitNode, b: &TransitNode) -> Result<Ordering, RowError> {
    // Equality first, exactly as datascript's `value-compare` does it. This is
    // not an optimisation: it is what makes a branch separator — which is a
    // COPY of its subtree's maximum — comparable to that maximum even when the
    // value is one of the hash-ordered kinds. It decides equality only, never
    // an order between two DIFFERENT such values, so the refusal below still
    // stands where it matters.
    if a == b {
        return Ok(Ordering::Equal);
    }
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb {
        return Ok(ra.cmp(&rb));
    }
    match (a, b) {
        (TransitNode::Bool(x), TransitNode::Bool(y)) => Ok(x.cmp(y)),
        (TransitNode::Str(x), TransitNode::Str(y)) => Ok(x.cmp(y)),
        (TransitNode::Uuid(x), TransitNode::Uuid(y)) => Ok(x.cmp(y)),
        (TransitNode::Keyword(x), TransitNode::Keyword(y)) => {
            Ok(keyword_key(x).cmp(&keyword_key(y)))
        }
        (TransitNode::Int(x), TransitNode::Int(y)) => Ok(x.cmp(y)),
        (
            TransitNode::Int(_) | TransitNode::Float(_),
            TransitNode::Int(_) | TransitNode::Float(_),
        ) => {
            let as_f64 = |n: &TransitNode| match n {
                TransitNode::Int(i) => *i as f64,
                TransitNode::Float(f) => f.get(),
                _ => unreachable!("guarded by the arm above"),
            };
            as_f64(a)
                .partial_cmp(&as_f64(b))
                .ok_or(RowError::ValueNotOrderable { kind: "NaN" })
        }
        // Epoch millis compare NUMERICALLY; "9" is not after "10".
        (TransitNode::InstantMillis(x), TransitNode::InstantMillis(y)) => {
            match (x.parse::<i64>(), y.parse::<i64>()) {
                (Ok(x), Ok(y)) => Ok(x.cmp(&y)),
                _ => Err(RowError::ValueNotOrderable {
                    kind: "unparseable instant",
                }),
            }
        }
        (TransitNode::Instant(x), TransitNode::Instant(y)) => Ok(x.cmp(y)),
        // One ISO instant and one epoch-millis instant are the same moment in
        // two spellings; ordering them needs a date parser this build has no
        // measurement for.
        (TransitNode::Instant(_), TransitNode::InstantMillis(_))
        | (TransitNode::InstantMillis(_), TransitNode::Instant(_)) => {
            Err(RowError::ValueNotOrderable {
                kind: "mixed instant representations",
            })
        }
        // Count first, then element-wise.
        (TransitNode::List(x), TransitNode::List(y)) => {
            if x.len() != y.len() {
                return Ok(x.len().cmp(&y.len()));
            }
            for (xi, yi) in x.iter().zip(y) {
                let ordering = compare_values(xi, yi)?;
                if ordering != Ordering::Equal {
                    return Ok(ordering);
                }
            }
            Ok(Ordering::Equal)
        }
        _ => Err(RowError::ValueNotOrderable { kind: kind_of(a) }),
    }
}

/// A node of an index tree.
///
/// Leaf and branch are told apart by the `addresses` column, which is the only
/// thing that distinguishes them on disk: both carry `:keys`, a branch's being
/// the maximum datom of each subtree rather than data.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf {
        keys: Vec<TreeDatom>,
    },
    Branch {
        keys: Vec<TreeDatom>,
        addresses: Vec<i64>,
    },
}

/// One index tree, loaded from a graph's rows.
#[derive(Debug)]
pub struct Tree<'a> {
    graph: &'a KvsGraph,
    index: Index,
    root: i64,
}

impl Node {
    /// Read one node out of the row at `addr`.
    ///
    /// The `addresses` COLUMN is what makes a node a branch. LogSeq strips
    /// `:addresses` from the content before encoding and re-attaches it from
    /// the column on restore, so the column is the only authority — a rule
    /// [`crate::kvs_writer::read_graph`] already enforces on the way in.
    fn parse(addr: i64, node: &TransitNode, addresses: Option<&[i64]>) -> Result<Self, RowError> {
        let TransitNode::Map(pairs) = node else {
            return Err(RowError::MalformedTreeNode {
                addr,
                detail: "it is not a Transit map".to_string(),
            });
        };
        let keys_node = pairs
            .iter()
            .find(|(k, _)| matches!(k, TransitNode::Keyword(name) if name == "keys"))
            .map(|(_, v)| v)
            .ok_or_else(|| RowError::MalformedTreeNode {
                addr,
                detail: "it carries no :keys".to_string(),
            })?;
        let TransitNode::List(tuples) = keys_node else {
            return Err(RowError::MalformedTreeNode {
                addr,
                detail: "its :keys is not a list".to_string(),
            });
        };
        let keys = tuples
            .iter()
            .map(|t| parse_datom(addr, t))
            .collect::<Result<Vec<_>, _>>()?;

        match addresses {
            None => Ok(Node::Leaf { keys }),
            Some(addresses) => {
                // The branch invariant: one separator per child, each the
                // maximum of its subtree. A mismatch means the tree we would
                // navigate is not the tree on disk.
                if keys.len() != addresses.len() {
                    return Err(RowError::MalformedTreeNode {
                        addr,
                        detail: format!(
                            "{} separator key(s) for {} child address(es); a branch must have one \
                             separator per child",
                            keys.len(),
                            addresses.len()
                        ),
                    });
                }
                Ok(Node::Branch {
                    keys,
                    addresses: addresses.to_vec(),
                })
            }
        }
    }
}

fn parse_datom(addr: i64, tuple: &TransitNode) -> Result<TreeDatom, RowError> {
    let TransitNode::List(slots) = tuple else {
        return Err(RowError::MalformedTreeNode {
            addr,
            detail: format!("a :keys entry is {tuple:?}, not a list"),
        });
    };
    let [
        TransitNode::Int(e),
        TransitNode::Keyword(a),
        v,
        TransitNode::Int(tx),
    ] = slots.as_slice()
    else {
        return Err(RowError::MalformedTreeNode {
            addr,
            detail: format!(
                "a :keys entry has {} slot(s), not [integer, keyword, value, integer]",
                slots.len()
            ),
        });
    };
    Ok(TreeDatom {
        e: *e,
        a: a.clone(),
        v: v.clone(),
        tx: *tx,
    })
}

impl<'a> Tree<'a> {
    pub fn load(graph: &'a KvsGraph, index: Index) -> Result<Self, RowError> {
        let root = index.root_addr(graph);
        let tree = Self { graph, index, root };
        // Read the root now so a bad root is an error from `load` rather than
        // from whichever traversal happens to touch it first.
        tree.node(root)?;
        Ok(tree)
    }

    /// Which index this is — and therefore how to order its datoms.
    ///
    /// A caller holding a tree should not have to carry the index alongside it
    /// to compare two of its datoms.
    pub fn index(&self) -> Index {
        self.index
    }

    /// The address this tree hangs from.
    pub fn root_addr(&self) -> i64 {
        self.root
    }

    fn node(&self, addr: i64) -> Result<Node, RowError> {
        let row = self
            .graph
            .rows
            .iter()
            .find(|r| r.addr == addr)
            .ok_or(RowError::MissingNode { addr })?;
        Node::parse(addr, &row.node, row.addresses.as_deref())
    }

    /// Every datom this index holds, in stored order.
    ///
    /// The leaves ARE the sorted sequence, so this is LogSeq's own ordering
    /// read back rather than anything this crate decided.
    pub fn datoms(&self) -> Result<Vec<TreeDatom>, RowError> {
        let mut out = Vec::new();
        self.walk(self.root, &mut out)?;
        Ok(out)
    }

    fn walk(&self, addr: i64, out: &mut Vec<TreeDatom>) -> Result<(), RowError> {
        match self.node(addr)? {
            Node::Leaf { keys } => out.extend(keys),
            Node::Branch { addresses, .. } => {
                for child in addresses {
                    self.walk(child, out)?;
                }
            }
        }
        Ok(())
    }

    /// Every branch's address and its children, top down.
    ///
    /// Whether a node HAS a left sibling decides which way `rotate` merges, and
    /// that is a question about parent grouping, not about leaf sizes.
    pub fn branches(&self) -> Result<Vec<(i64, Vec<i64>)>, RowError> {
        let mut out = Vec::new();
        self.collect_branches(self.root, &mut out)?;
        Ok(out)
    }

    fn collect_branches(&self, addr: i64, out: &mut Vec<(i64, Vec<i64>)>) -> Result<(), RowError> {
        if let Node::Branch { addresses, .. } = self.node(addr)? {
            out.push((addr, addresses.clone()));
            for child in addresses {
                self.collect_branches(child, out)?;
            }
        }
        Ok(())
    }

    /// Every leaf's address and key count, left to right.
    ///
    /// The partition itself: two writers holding the same datoms in the same
    /// order can still split them between leaves differently, and this is the
    /// view that shows it.
    pub fn leaf_lengths(&self) -> Result<Vec<(i64, usize)>, RowError> {
        let mut out = Vec::new();
        self.collect_leaves(self.root, &mut out)?;
        Ok(out)
    }

    fn collect_leaves(&self, addr: i64, out: &mut Vec<(i64, usize)>) -> Result<(), RowError> {
        match self.node(addr)? {
            Node::Leaf { keys } => out.push((addr, keys.len())),
            Node::Branch { addresses, .. } => {
                for child in addresses {
                    self.collect_leaves(child, out)?;
                }
            }
        }
        Ok(())
    }

    /// How many levels the tree has, leaves included.
    ///
    /// Measured to equal `shift + 1`; a B+-tree keeps every leaf at the same
    /// depth, so following the first child is enough.
    pub fn depth(&self) -> Result<usize, RowError> {
        let mut depth = 1;
        let mut addr = self.root;
        while let Node::Branch { addresses, .. } = self.node(addr)? {
            addr = *addresses.first().ok_or(RowError::MalformedTreeNode {
                addr,
                detail: "a branch with no children".to_string(),
            })?;
            depth += 1;
        }
        Ok(depth)
    }
}

// ---------------------------------------------------------- editing a tree

/// The branching factor this build is pinned to, as a length.
const MAX_LEN: usize = crate::kvs_writer::PINNED_BRANCHING_FACTOR as usize;
/// A node at or below this length is merged rather than borrowed from.
const MIN_LEN: usize = MAX_LEN / 2;

type NodeId = usize;

#[derive(Debug, Clone)]
enum WorkNode {
    Leaf {
        addr: Option<i64>,
        keys: Vec<TreeDatom>,
    },
    Branch {
        addr: Option<i64>,
        keys: Vec<TreeDatom>,
        children: Vec<NodeId>,
    },
}

impl WorkNode {
    fn keys(&self) -> &[TreeDatom] {
        match self {
            WorkNode::Leaf { keys, .. } | WorkNode::Branch { keys, .. } => keys,
        }
    }

    /// The node's maximum — its last key. A branch separator IS this value for
    /// the subtree below it, which is why taking a maximum never compares two
    /// existing keys.
    fn lim_key(&self) -> Result<&TreeDatom, RowError> {
        self.keys().last().ok_or(RowError::MalformedTreeNode {
            addr: -1,
            detail: "an empty node has no maximum".to_string(),
        })
    }

    fn len(&self) -> usize {
        self.keys().len()
    }
}

/// An index tree open for modification.
///
/// Strictly incremental by construction: datoms are inserted into and removed
/// from the existing nodes, and the sequence is never re-sorted. That is
/// invariant (2) of docs/Testing/LogseqDbTreeOrder.md, and it is what keeps
/// every comparison decidable without ClojureScript's hash.
///
/// Addresses follow LogSeq's measured behaviour: a node that is MODIFIED keeps
/// the address it already had, and only a node that is genuinely NEW — the
/// second half of a split — is left addressless for the writer to allocate
/// above `:max-addr`. Nodes that merge away are simply abandoned; LogSeq's own
/// storage layer discards its delete list too, which is why a flushed graph
/// carries unreferenced rows.
#[derive(Debug)]
pub struct EditableTree {
    index: Index,
    arena: Vec<WorkNode>,
    root: NodeId,
}

impl EditableTree {
    /// Read a whole index into memory.
    pub fn load(graph: &KvsGraph, index: Index) -> Result<Self, RowError> {
        let tree = Tree::load(graph, index)?;
        let mut arena = Vec::new();
        let root = load_into(&tree, tree.root_addr(), &mut arena)?;
        Ok(Self { index, arena, root })
    }

    pub fn index(&self) -> Index {
        self.index
    }

    fn alloc(&mut self, node: WorkNode) -> NodeId {
        self.arena.push(node);
        self.arena.len() - 1
    }

    /// `binary-search-l`: the leftmost index in `[0, r]` whose key is >= `k`,
    /// or `r + 1` when every one of them is smaller.
    fn search_l(&self, keys: &[TreeDatom], r: isize, k: &TreeDatom) -> Result<usize, RowError> {
        let mut lo: isize = 0;
        let mut hi = r;
        while lo <= hi {
            let mid = ((lo + hi) as usize) >> 1;
            if self.index.compare(&keys[mid], k)? == Ordering::Less {
                lo = mid as isize + 1;
            } else {
                hi = mid as isize - 1;
            }
        }
        Ok(lo as usize)
    }

    /// How many levels the edited tree has, leaves included.
    pub fn depth(&self) -> Result<usize, RowError> {
        let mut depth = 1;
        let mut id = self.root;
        while let WorkNode::Branch { children, .. } = &self.arena[id] {
            id = *children.first().ok_or(RowError::MalformedTreeNode {
                addr: -1,
                detail: "a branch with no children".to_string(),
            })?;
            depth += 1;
        }
        Ok(depth)
    }

    /// Every datom in the tree, in order — the edited sequence.
    pub fn datoms(&self) -> Vec<TreeDatom> {
        let mut out = Vec::new();
        self.collect(self.root, &mut out);
        out
    }

    fn collect(&self, id: NodeId, out: &mut Vec<TreeDatom>) {
        match &self.arena[id] {
            WorkNode::Leaf { keys, .. } => out.extend(keys.iter().cloned()),
            WorkNode::Branch { children, .. } => {
                for child in children.clone() {
                    self.collect(child, out);
                }
            }
        }
    }
}

fn load_into(tree: &Tree<'_>, addr: i64, arena: &mut Vec<WorkNode>) -> Result<NodeId, RowError> {
    Ok(match tree.node(addr)? {
        Node::Leaf { keys } => {
            arena.push(WorkNode::Leaf {
                addr: Some(addr),
                keys,
            });
            arena.len() - 1
        }
        Node::Branch { keys, addresses } => {
            let children = addresses
                .iter()
                .map(|child| load_into(tree, *child, arena))
                .collect::<Result<Vec<_>, _>>()?;
            arena.push(WorkNode::Branch {
                addr: Some(addr),
                keys,
                children,
            });
            arena.len() - 1
        }
    })
}

impl EditableTree {
    /// Insert one datom. `false` when it was already present.
    pub fn insert(&mut self, datom: &TreeDatom) -> Result<bool, RowError> {
        let Some(nodes) = self.node_conj(self.root, datom)? else {
            return Ok(false);
        };
        self.root = match nodes.as_slice() {
            [only] => *only,
            [first, second] => {
                let keys = vec![
                    self.arena[*first].lim_key()?.clone(),
                    self.arena[*second].lim_key()?.clone(),
                ];
                // A split root grows the tree by a level. The new root is a NEW
                // node, so it takes a fresh address and addr 0's pointer moves.
                self.alloc(WorkNode::Branch {
                    addr: None,
                    keys,
                    children: vec![*first, *second],
                })
            }
            other => {
                return Err(RowError::MalformedTreeNode {
                    addr: -1,
                    detail: format!("an insert returned {} nodes, not 1 or 2", other.len()),
                });
            }
        };
        Ok(true)
    }

    fn node_conj(&mut self, id: NodeId, d: &TreeDatom) -> Result<Option<Vec<NodeId>>, RowError> {
        match self.arena[id].clone() {
            WorkNode::Leaf { addr, keys } => {
                let idx = self.search_l(&keys, keys.len() as isize - 1, d)?;
                if idx < keys.len() && self.index.compare(d, &keys[idx])? == Ordering::Equal {
                    return Ok(None);
                }
                if keys.len() < MAX_LEN {
                    let mut keys = keys;
                    keys.insert(idx, d.clone());
                    self.arena[id] = WorkNode::Leaf { addr, keys };
                    return Ok(Some(vec![id]));
                }
                // Full: split. `half(len + 1)` is 16 at a branching factor of
                // 32, and the FIRST half keeps this node's address.
                let middle = (keys.len() + 1) >> 1;
                let (first, second) = if idx > middle {
                    let mut second = keys[middle..].to_vec();
                    second.insert(idx - middle, d.clone());
                    (keys[..middle].to_vec(), second)
                } else {
                    let mut first = keys[..middle].to_vec();
                    first.insert(idx, d.clone());
                    (first, keys[middle..].to_vec())
                };
                self.arena[id] = WorkNode::Leaf { addr, keys: first };
                let sibling = self.alloc(WorkNode::Leaf {
                    addr: None,
                    keys: second,
                });
                Ok(Some(vec![id, sibling]))
            }
            WorkNode::Branch {
                addr,
                keys,
                children,
            } => {
                // Searching to `len - 2` clamps to the last child, so a datom
                // past every separator still descends somewhere.
                let idx = self.search_l(&keys, keys.len() as isize - 2, d)?;
                let Some(nodes) = self.node_conj(children[idx], d)? else {
                    return Ok(None);
                };
                let limits = self.limits(&nodes)?;
                let mut children = children;
                let mut keys = keys;
                children.splice(idx..idx + 1, nodes.iter().copied());
                keys.splice(idx..idx + 1, limits);

                if children.len() <= MAX_LEN {
                    self.arena[id] = WorkNode::Branch {
                        addr,
                        keys,
                        children,
                    };
                    return Ok(Some(vec![id]));
                }
                let middle = children.len() >> 1;
                let sibling = self.alloc(WorkNode::Branch {
                    addr: None,
                    keys: keys[middle..].to_vec(),
                    children: children[middle..].to_vec(),
                });
                self.arena[id] = WorkNode::Branch {
                    addr,
                    keys: keys[..middle].to_vec(),
                    children: children[..middle].to_vec(),
                };
                Ok(Some(vec![id, sibling]))
            }
        }
    }

    fn limits(&self, nodes: &[NodeId]) -> Result<Vec<TreeDatom>, RowError> {
        nodes
            .iter()
            .map(|n| self.arena[*n].lim_key().cloned())
            .collect()
    }

    /// Remove one datom. `false` when it was not there.
    pub fn remove(&mut self, datom: &TreeDatom) -> Result<bool, RowError> {
        let Some(nodes) = self.node_disj(self.root, datom, true, None, None)? else {
            return Ok(false);
        };
        // At the root, `rotate` returns the node unchanged — a root never
        // merges, so there is exactly one.
        self.root = *nodes.first().ok_or(RowError::MalformedTreeNode {
            addr: -1,
            detail: "removing from the root returned no node".to_string(),
        })?;

        // A root left with a single child is replaced by that child, so the
        // tree LOSES A LEVEL. MEASURED, not assumed: growing a storage-backed
        // graph to 1500 datoms gives shift 2, and retracting down to 500 then
        // to 3 gives shift 1 then 0 — and shift is depth - 1. A writer that
        // skipped this would keep building deeper trees than LogSeq's for the
        // same datoms. One level at a time, which is why this is a loop.
        while let WorkNode::Branch { children, .. } = &self.arena[self.root] {
            match children.as_slice() {
                [only] => self.root = *only,
                _ => break,
            }
        }
        Ok(true)
    }

    fn node_disj(
        &mut self,
        id: NodeId,
        d: &TreeDatom,
        root: bool,
        left: Option<NodeId>,
        right: Option<NodeId>,
    ) -> Result<Option<Vec<NodeId>>, RowError> {
        match self.arena[id].clone() {
            WorkNode::Leaf { addr, keys } => {
                let idx = self.search_l(&keys, keys.len() as isize - 1, d)?;
                if idx >= keys.len() || self.index.compare(&keys[idx], d)? != Ordering::Equal {
                    return Ok(None);
                }
                let mut keys = keys;
                keys.remove(idx);
                self.arena[id] = WorkNode::Leaf { addr, keys };
                self.rotate(id, root, left, right).map(Some)
            }
            WorkNode::Branch {
                addr,
                keys,
                children,
            } => {
                let idx = self.search_l(&keys, keys.len() as isize - 1, d)?;
                if idx >= keys.len() {
                    // Past this subtree's maximum: the datom is not here.
                    return Ok(None);
                }
                let child_left = (idx > 0).then(|| children[idx - 1]);
                let child_right = (idx + 1 < children.len()).then(|| children[idx + 1]);
                let Some(replacement) =
                    self.node_disj(children[idx], d, false, child_left, child_right)?
                else {
                    return Ok(None);
                };

                // `rotate` returned the replacement for [left, child, right],
                // so the splice covers whichever of those existed.
                let from = if child_left.is_some() { idx - 1 } else { idx };
                let to = if child_right.is_some() {
                    idx + 2
                } else {
                    idx + 1
                };
                let limits = self.limits(&replacement)?;
                let mut children = children;
                let mut keys = keys;
                children.splice(from..to, replacement.iter().copied());
                keys.splice(from..to, limits);

                self.arena[id] = WorkNode::Branch {
                    addr,
                    keys,
                    children,
                };
                self.rotate(id, root, left, right).map(Some)
            }
        }
    }

    /// Restore the minimum-occupancy invariant, returning the replacement for
    /// `[left, id, right]`.
    ///
    /// A merged-away node is simply abandoned: LogSeq's storage layer ignores
    /// its own delete list, which is why a flushed graph keeps unreferenced
    /// rows. Merging keeps the FIRST node's address, so the survivor is an
    /// update rather than a new row.
    fn rotate(
        &mut self,
        id: NodeId,
        root: bool,
        left: Option<NodeId>,
        right: Option<NodeId>,
    ) -> Result<Vec<NodeId>, RowError> {
        if root {
            return Ok(vec![id]);
        }
        if self.arena[id].len() > MIN_LEN {
            return Ok([left, Some(id), right].into_iter().flatten().collect());
        }
        if let Some(l) = left {
            if self.arena[l].len() <= MIN_LEN {
                let merged = self.merge(l, id)?;
                return Ok([Some(merged), right].into_iter().flatten().collect());
            }
        }
        if let Some(r) = right {
            if self.arena[r].len() <= MIN_LEN {
                let merged = self.merge(id, r)?;
                return Ok([left, Some(merged)].into_iter().flatten().collect());
            }
        }
        // Redistribute with the SMALLER sibling, ties going RIGHT — not with
        // the left one by preference. MEASURED: on the fixture, a leaf of 17
        // losing a datom sits between two siblings of 23; LogSeq redistributes
        // it rightwards, which a left-first rule cannot produce. The same rule
        // explains the other divergence, where left=18 and right=17 and the
        // rightward redistribution happens to leave the partition looking
        // untouched.
        let len = |id: NodeId| self.arena[id].len();
        if let Some(l) = left {
            if right.is_none_or(|r| len(l) < len(r)) {
                let (a, b) = self.merge_n_split(l, id)?;
                return Ok([Some(a), Some(b), right].into_iter().flatten().collect());
            }
        }
        if let Some(r) = right {
            let (a, b) = self.merge_n_split(id, r)?;
            return Ok([left, Some(a), Some(b)].into_iter().flatten().collect());
        }
        // No siblings and not the root: only reachable from a branch with a
        // single child, which this writer never builds.
        Ok(vec![id])
    }

    /// Fold `b` into `a`. `a` keeps its address; `b` is abandoned.
    fn merge(&mut self, a: NodeId, b: NodeId) -> Result<NodeId, RowError> {
        let merged = match (self.arena[a].clone(), self.arena[b].clone()) {
            (
                WorkNode::Leaf { addr, keys },
                WorkNode::Leaf {
                    keys: other_keys, ..
                },
            ) => WorkNode::Leaf {
                addr,
                keys: [keys, other_keys].concat(),
            },
            (
                WorkNode::Branch {
                    addr,
                    keys,
                    children,
                },
                WorkNode::Branch {
                    keys: other_keys,
                    children: other_children,
                    ..
                },
            ) => WorkNode::Branch {
                addr,
                keys: [keys, other_keys].concat(),
                children: [children, other_children].concat(),
            },
            _ => {
                return Err(RowError::MalformedTreeNode {
                    addr: -1,
                    detail: "a leaf and a branch cannot merge; the tree is not level".to_string(),
                });
            }
        };
        self.arena[a] = merged;
        Ok(a)
    }

    /// Redistribute `a` and `b` evenly. Each keeps its own address.
    fn merge_n_split(&mut self, a: NodeId, b: NodeId) -> Result<(NodeId, NodeId), RowError> {
        match (self.arena[a].clone(), self.arena[b].clone()) {
            (
                WorkNode::Leaf { addr, keys },
                WorkNode::Leaf {
                    addr: other_addr,
                    keys: other_keys,
                },
            ) => {
                let all = [keys, other_keys].concat();
                let cut = all.len() >> 1;
                self.arena[a] = WorkNode::Leaf {
                    addr,
                    keys: all[..cut].to_vec(),
                };
                self.arena[b] = WorkNode::Leaf {
                    addr: other_addr,
                    keys: all[cut..].to_vec(),
                };
                Ok((a, b))
            }
            (
                WorkNode::Branch {
                    addr,
                    keys,
                    children,
                },
                WorkNode::Branch {
                    addr: other_addr,
                    keys: other_keys,
                    children: other_children,
                },
            ) => {
                let all_keys = [keys, other_keys].concat();
                let all_children = [children, other_children].concat();
                let cut = all_keys.len() >> 1;
                self.arena[a] = WorkNode::Branch {
                    addr,
                    keys: all_keys[..cut].to_vec(),
                    children: all_children[..cut].to_vec(),
                };
                self.arena[b] = WorkNode::Branch {
                    addr: other_addr,
                    keys: all_keys[cut..].to_vec(),
                    children: all_children[cut..].to_vec(),
                };
                Ok((a, b))
            }
            _ => Err(RowError::MalformedTreeNode {
                addr: -1,
                detail: "a leaf and a branch cannot redistribute; the tree is not level"
                    .to_string(),
            }),
        }
    }
}

impl EditableTree {
    /// Everything a B+-tree of this shape must still be true about.
    ///
    /// Cheap to run and worth running after every edit in tests: a split or
    /// merge that put a datom in the wrong node still produces a readable
    /// tree, and without these checks the first sign of it would be LogSeq
    /// quietly losing datoms.
    pub fn check_invariants(&self) -> Result<(), RowError> {
        let bad = |detail: String| RowError::MalformedTreeNode { addr: -1, detail };
        let mut leaf_depths = Vec::new();
        self.check_node(self.root, true, 1, &mut leaf_depths)?;

        // A single-child root means a level that should have been collapsed
        // away — measured behaviour, so its absence is a defect, not a style.
        if let WorkNode::Branch { children, .. } = &self.arena[self.root] {
            if children.len() == 1 {
                return Err(bad(
                    "the root is a branch with one child; that level should have collapsed"
                        .to_string(),
                ));
            }
        }

        if leaf_depths.windows(2).any(|w| w[0] != w[1]) {
            return Err(bad(format!(
                "leaves sit at different depths ({:?}..); a B+-tree keeps them level",
                &leaf_depths[..leaf_depths.len().min(8)]
            )));
        }
        Ok(())
    }

    fn check_node(
        &self,
        id: NodeId,
        root: bool,
        depth: usize,
        leaf_depths: &mut Vec<usize>,
    ) -> Result<(), RowError> {
        let bad = |detail: String| RowError::MalformedTreeNode { addr: -1, detail };
        let node = &self.arena[id];
        if node.len() > MAX_LEN {
            return Err(bad(format!(
                "a node holds {} keys, over the branching factor {MAX_LEN}",
                node.len()
            )));
        }
        if !root && node.len() == 0 {
            return Err(bad("an empty non-root node".to_string()));
        }
        match node {
            WorkNode::Leaf { .. } => leaf_depths.push(depth),
            WorkNode::Branch { keys, children, .. } => {
                if keys.len() != children.len() {
                    return Err(bad(format!(
                        "a branch has {} separators for {} children",
                        keys.len(),
                        children.len()
                    )));
                }
                for (i, child) in children.iter().enumerate() {
                    // The separator IS the subtree's maximum. Comparing it to
                    // the child's own last key never compares two existing
                    // values against each other, so it stays decidable.
                    let child_max = self.subtree_max(*child)?;
                    if self.index.compare(&keys[i], &child_max)? != Ordering::Equal {
                        return Err(bad(format!(
                            "separator {i} is not its subtree's maximum: {:?} vs {child_max:?}",
                            keys[i]
                        )));
                    }
                    self.check_node(*child, false, depth + 1, leaf_depths)?;
                }
            }
        }
        Ok(())
    }

    fn subtree_max(&self, id: NodeId) -> Result<TreeDatom, RowError> {
        match &self.arena[id] {
            WorkNode::Leaf { keys, .. } => {
                keys.last()
                    .cloned()
                    .ok_or_else(|| RowError::MalformedTreeNode {
                        addr: -1,
                        detail: "an empty leaf has no maximum".to_string(),
                    })
            }
            WorkNode::Branch { children, .. } => {
                let last = children.last().ok_or(RowError::MalformedTreeNode {
                    addr: -1,
                    detail: "a branch with no children".to_string(),
                })?;
                self.subtree_max(*last)
            }
        }
    }
}

/// One node ready to be written as a `kvs` row.
#[derive(Debug, Clone, PartialEq)]
pub struct SerializedNode {
    pub addr: i64,
    /// The node's Transit content. Child pointers are NOT in here — LogSeq's
    /// storage layer strips them before encoding.
    pub node: TransitNode,
    /// The `addresses` column: `Some` for a branch, `None` for a leaf.
    pub addresses: Option<Vec<i64>>,
}

/// What a serialization produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Serialized {
    pub nodes: Vec<SerializedNode>,
    /// The address addr 0 must now point this index at.
    pub root_addr: i64,
    /// The highest address handed out, so `:max-addr` can be advanced.
    pub max_addr: i64,
}

fn datom_tuple(d: &TreeDatom) -> TransitNode {
    TransitNode::List(vec![
        TransitNode::Int(d.e),
        TransitNode::Keyword(d.a.clone()),
        d.v.clone(),
        TransitNode::Int(d.tx),
    ])
}

impl EditableTree {
    /// Emit every REACHABLE node as a row.
    ///
    /// Addresses follow LogSeq's measured policy: a node that already had one
    /// keeps it — so a modified node is an UPDATE at its old address — and only
    /// a node created by a split is given a fresh address above `max_addr`.
    /// Nodes that merged away are simply not emitted; they stay in the table
    /// as unreferenced rows, exactly as LogSeq leaves them.
    ///
    /// Children are assigned addresses before their parents, because a branch
    /// cannot be written until it knows where its children live.
    pub fn serialize(&self, max_addr: i64) -> Result<Serialized, RowError> {
        let mut next = max_addr;
        let mut assigned = std::collections::HashMap::new();
        self.assign(self.root, &mut next, &mut assigned);

        let mut nodes = Vec::new();
        self.emit(self.root, &assigned, &mut nodes)?;
        Ok(Serialized {
            root_addr: assigned[&self.root],
            nodes,
            max_addr: next,
        })
    }

    fn assign(&self, id: NodeId, next: &mut i64, out: &mut std::collections::HashMap<NodeId, i64>) {
        if let WorkNode::Branch { children, .. } = &self.arena[id] {
            for child in children {
                self.assign(*child, next, out);
            }
        }
        let addr = match &self.arena[id] {
            WorkNode::Leaf { addr, .. } | WorkNode::Branch { addr, .. } => *addr,
        };
        let addr = addr.unwrap_or_else(|| {
            *next += 1;
            *next
        });
        out.insert(id, addr);
    }

    fn emit(
        &self,
        id: NodeId,
        assigned: &std::collections::HashMap<NodeId, i64>,
        out: &mut Vec<SerializedNode>,
    ) -> Result<(), RowError> {
        let keys = TransitNode::List(self.arena[id].keys().iter().map(datom_tuple).collect());
        let content = TransitNode::Map(vec![(TransitNode::Keyword("keys".into()), keys)]);

        let addresses = match &self.arena[id] {
            WorkNode::Leaf { .. } => None,
            WorkNode::Branch { children, .. } => {
                for child in children {
                    self.emit(*child, assigned, out)?;
                }
                Some(children.iter().map(|c| assigned[c]).collect())
            }
        };
        out.push(SerializedNode {
            addr: assigned[&id],
            node: content,
            addresses,
        });
        Ok(())
    }
}

impl EditableTree {
    /// Find the stored datom matching `probe` on `(e, a, v)`, whatever its
    /// transaction id.
    ///
    /// A retraction in the tail carries the NEW transaction's id, while the
    /// datom it retracts still carries the id of whatever transaction first
    /// asserted it. So a retraction identifies its target by value, not by
    /// transaction — matching by the full key would silently retract nothing.
    ///
    /// The descent probes with `tx = 0`, which sorts at or before every real
    /// datom sharing that `(e, a, v)`, and then checks whether what it landed
    /// on is actually the one.
    pub fn find_ignoring_tx(&self, probe: &TreeDatom) -> Result<Option<TreeDatom>, RowError> {
        let floor = TreeDatom {
            e: probe.e,
            a: probe.a.clone(),
            v: probe.v.clone(),
            tx: 0,
        };
        let mut id = self.root;
        loop {
            match &self.arena[id] {
                WorkNode::Leaf { keys, .. } => {
                    let idx = self.search_l(keys, keys.len() as isize - 1, &floor)?;
                    return Ok(keys
                        .get(idx)
                        .filter(|found| {
                            found.e == probe.e && found.a == probe.a && found.v == probe.v
                        })
                        .cloned());
                }
                WorkNode::Branch { keys, children, .. } => {
                    let idx = self.search_l(keys, keys.len() as isize - 2, &floor)?;
                    id = children[idx];
                }
            }
        }
    }
}
