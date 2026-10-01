//! Differential PBT of the stable-id index: the reference model IS the full
//! tree scan (every live node with a `STABLE_ID`, smallest `TreeID` first).
//!
//! Invariant I-IDX, after every transition and after each op inside a write
//! batch: a lookup answers the scan's smallest carrier, and a miss means the
//! scan holds no carrier. Checked through the backend's lookup and through the
//! doc's index; outside a batch the index's whole live set equals the scan.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::repository::CoreOperations;
use holon_api::repository::NewBlock;
use holon_loro::LoroDocument;
use holon_loro::STABLE_ID;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::write_stable_id;
use loro::ExportMode;
use loro::Frontiers;
use loro::LoroTree;
use loro::TreeID;
use loro::TreeParentId;
use proptest::prelude::*;
use proptest::sample::Index;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const POOL: usize = 40;
/// Past the index's touched-node limit, so the import trips a full rebuild.
const LARGE_IMPORT: usize = 1100;
const SAMPLED: usize = 20;

static NEXT_PEER: AtomicU64 = AtomicU64::new(1);

fn fresh_peer() -> u64 {
    NEXT_PEER.fetch_add(1, Ordering::SeqCst)
}

fn sid(i: usize) -> String {
    format!("s{}", i % POOL)
}

type Scan = BTreeMap<String, Vec<TreeID>>;

fn scan_tree(tree: &LoroTree) -> Scan {
    let mut found = Scan::new();
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        let meta = tree.get_meta(node.id).expect("a live node has meta");
        if let Some(value) = meta.get(STABLE_ID) {
            let value = value
                .into_value()
                .expect("STABLE_ID is a value, not a container");
            let id = value
                .into_string()
                .expect("STABLE_ID is a string")
                .to_string();
            found.entry(id).or_default().push(node.id);
        }
    }
    for nodes in found.values_mut() {
        nodes.sort();
    }
    found
}

/// Live settled nodes, ascending: the domain index picks resolve against.
fn live_nodes(scan: &Scan) -> Vec<(TreeID, String)> {
    let mut nodes: Vec<(TreeID, String)> = scan
        .iter()
        .flat_map(|(id, nodes)| nodes.iter().map(move |n| (*n, id.clone())))
        .collect();
    nodes.sort();
    nodes
}

fn pick<T: Clone>(items: &[T], at: &Index) -> Option<T> {
    (!items.is_empty()).then(|| items[at.index(items.len())].clone())
}

fn subtree_contains(tree: &LoroTree, root: TreeID, node: TreeID) -> bool {
    let mut at = Some(node);
    while let Some(n) = at {
        if n == root {
            return true;
        }
        at = match tree.parent(n) {
            Some(TreeParentId::Node(p)) => Some(p),
            _ => None,
        };
    }
    false
}

#[derive(Debug, Clone)]
enum BatchOp {
    /// `parent`: `None` = root; `Some(i)` picks among the existing live nodes
    /// AND the nodes this batch created so far (the latter first).
    Create {
        sid: usize,
        parent: Option<Index>,
    },
    Rewrite {
        node: Index,
        sid: usize,
    },
    Delete {
        node: Index,
    },
    /// A raw move of a deleted node's child to the root, which revives it.
    Revive {
        node: Index,
    },
    Revert {
        to: Index,
    },
}

#[derive(Debug, Clone)]
enum Transition {
    Create {
        sid: usize,
        parent: Option<Index>,
    },
    /// One `create_blocks` call whose blocks each name the previous one as
    /// parent: a parent created in the same batch.
    CreateChain {
        sids: Vec<usize>,
        parent: Option<Index>,
    },
    Delete {
        node: Index,
    },
    Move {
        node: Index,
        to: Option<Index>,
    },
    RewriteId {
        node: Index,
        sid: usize,
    },
    /// Peer B creates `sid` (it may already be live in A) and A imports it.
    PeerCreate {
        sid: usize,
        parent: Option<Index>,
    },
    /// B moves `mover` into `victim` (`into`) or a child of `victim` to the
    /// root, while A deletes `victim`; then A imports B.
    ConcurrentDeleteMove {
        victim: Index,
        mover: Index,
        into: bool,
    },
    LargeImport,
    HalfBorn {
        parent: Option<Index>,
    },
    Settle {
        node: Index,
        sid: usize,
    },
    Batch {
        ops: Vec<BatchOp>,
    },
    Reload,
    Revert {
        to: Index,
    },
}

#[derive(Debug, Clone, Default)]
struct Ref;

fn opt_index() -> impl Strategy<Value = Option<Index>> {
    proptest::option::weighted(0.7, any::<Index>())
}

impl ReferenceStateMachine for Ref {
    type State = Ref;
    type Transition = Transition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(Ref).boxed()
    }

    fn transitions(_: &Self::State) -> BoxedStrategy<Self::Transition> {
        let batch_op = prop_oneof![
            3 => (0..POOL, opt_index()).prop_map(|(sid, parent)| BatchOp::Create { sid, parent }),
            1 => (any::<Index>(), 0..POOL).prop_map(|(node, sid)| BatchOp::Rewrite { node, sid }),
            1 => any::<Index>().prop_map(|node| BatchOp::Delete { node }),
            1 => any::<Index>().prop_map(|node| BatchOp::Revive { node }),
            1 => any::<Index>().prop_map(|to| BatchOp::Revert { to }),
        ];
        prop_oneof![
            6 => (0..POOL, opt_index()).prop_map(|(sid, parent)| Transition::Create { sid, parent }),
            2 => (proptest::collection::vec(0..POOL, 2..4), opt_index())
                .prop_map(|(sids, parent)| Transition::CreateChain { sids, parent }),
            2 => any::<Index>().prop_map(|node| Transition::Delete { node }),
            2 => (any::<Index>(), opt_index()).prop_map(|(node, to)| Transition::Move { node, to }),
            2 => (any::<Index>(), 0..POOL)
                .prop_map(|(node, sid)| Transition::RewriteId { node, sid }),
            3 => (0..POOL, opt_index())
                .prop_map(|(sid, parent)| Transition::PeerCreate { sid, parent }),
            2 => (any::<Index>(), any::<Index>(), any::<bool>()).prop_map(
                |(victim, mover, into)| Transition::ConcurrentDeleteMove { victim, mover, into }
            ),
            1 => Just(Transition::LargeImport),
            1 => opt_index().prop_map(|parent| Transition::HalfBorn { parent }),
            1 => (any::<Index>(), 0..POOL).prop_map(|(node, sid)| Transition::Settle { node, sid }),
            3 => proptest::collection::vec(batch_op, 1..6).prop_map(|ops| Transition::Batch { ops }),
            1 => Just(Transition::Reload),
            1 => any::<Index>().prop_map(|to| Transition::Revert { to }),
        ]
        .boxed()
    }

    fn apply(state: Self::State, _: &Self::Transition) -> Self::State {
        state
    }
}

struct Sut {
    rt: tokio::runtime::Runtime,
    doc: Arc<LoroDocument>,
    backend: LoroBackend,
    half_born: Vec<TreeID>,
    history: Vec<Frontiers>,
    large_imports: usize,
    fresh_ids: usize,
}

impl Sut {
    fn new() -> Self {
        let doc = Arc::new(
            LoroDocument::new_with_peer_id("stable-id-pbt".into(), Some(fresh_peer())).unwrap(),
        );
        Self {
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            backend: LoroBackend::from_document(doc.clone()),
            doc,
            half_born: Vec::new(),
            history: Vec::new(),
            large_imports: 0,
            fresh_ids: 0,
        }
    }

    fn scan(&self) -> Scan {
        self.doc
            .with_read(|d| Ok(scan_tree(&d.get_tree(TREE_NAME))))
            .unwrap()
    }

    /// A pool id no live node carries; a fresh id when all are taken.
    fn free_sid(&mut self, scan: &Scan, wanted: usize) -> String {
        (0..POOL)
            .map(|k| sid(wanted + k))
            .find(|s| !scan.contains_key(s))
            .unwrap_or_else(|| {
                self.fresh_ids += 1;
                format!("fresh{}", self.fresh_ids)
            })
    }

    fn parent_uri(scan: &Scan, parent: &Option<Index>) -> EntityUri {
        match parent.as_ref().and_then(|at| pick(&live_nodes(scan), at)) {
            Some((_, id)) => EntityUri::block(&id),
            None => EntityUri::no_parent(),
        }
    }

    /// A peer that starts from A's current state.
    fn peer(&self) -> LoroDocument {
        let peer = LoroDocument::new_with_peer_id("stable-id-pbt-peer".into(), Some(fresh_peer()))
            .unwrap();
        peer.apply_update(&self.doc.export_snapshot().unwrap())
            .unwrap();
        peer
    }

    fn import_from(&self, peer: &LoroDocument) {
        let since = self.doc.with_read(|d| Ok(d.oplog_vv())).unwrap();
        let update = peer
            .with_read(|d| Ok(d.export(ExportMode::updates(&since))?))
            .unwrap();
        self.doc.apply_update(&update).unwrap();
    }

    fn apply(&mut self, transition: &Transition) {
        let scan = self.scan();
        let live = live_nodes(&scan);
        match transition {
            Transition::Create { sid, parent } => {
                let id = self.free_sid(&scan, *sid);
                let parent = Self::parent_uri(&scan, parent);
                self.rt
                    .block_on(self.backend.create_block(
                        parent,
                        BlockContent::text("x"),
                        Some(EntityUri::block(&id)),
                    ))
                    .unwrap();
            }
            Transition::CreateChain { sids, parent } => {
                let mut taken = scan.clone();
                let mut parent = Self::parent_uri(&scan, parent);
                let mut blocks = Vec::new();
                for s in sids {
                    let id = self.free_sid(&taken, *s);
                    taken.insert(id.clone(), Vec::new());
                    let mut block = NewBlock::text(parent.clone(), "x");
                    block.id = Some(EntityUri::block(&id));
                    parent = EntityUri::block(&id);
                    blocks.push(block);
                }
                self.rt
                    .block_on(self.backend.create_blocks(blocks))
                    .unwrap();
            }
            Transition::Delete { node } => {
                if let Some((_, id)) = pick(&live, node) {
                    self.rt
                        .block_on(self.backend.delete_block(&format!("block:{id}")))
                        .unwrap();
                }
            }
            Transition::Move { node, to } => {
                let Some((mover, id)) = pick(&live, node) else {
                    return;
                };
                let targets: Vec<(TreeID, String)> = self
                    .doc
                    .with_read(|d| {
                        let tree = d.get_tree(TREE_NAME);
                        Ok(live
                            .iter()
                            .filter(|(n, _)| !subtree_contains(&tree, mover, *n))
                            .cloned()
                            .collect())
                    })
                    .unwrap();
                // Which carrier a duplicated id names is the lookup under
                // test, so a move only takes unique ids: the cycle check above
                // must see the nodes the backend moves.
                if scan[&id].len() > 1 {
                    return;
                }
                let parent = match to.as_ref().and_then(|at| pick(&targets, at)) {
                    Some((_, tid)) if scan[&tid].len() == 1 => EntityUri::block(&tid),
                    Some(_) => return,
                    None => EntityUri::no_parent(),
                };
                self.rt
                    .block_on(
                        self.backend
                            .move_block(&EntityUri::block(&id), parent, None),
                    )
                    .unwrap();
            }
            Transition::RewriteId { node, sid } => {
                let Some((n, id)) = pick(&live, node) else {
                    return;
                };
                if scan[&id][0] != n {
                    return;
                }
                let new_id = self.free_sid(&scan, *sid);
                self.rt
                    .block_on(
                        self.backend
                            .set_external_id(&format!("block:{id}"), &format!("block:{new_id}")),
                    )
                    .unwrap();
            }
            Transition::PeerCreate { sid: s, parent } => {
                let parent = parent.as_ref().and_then(|at| pick(&live, at)).map(|p| p.0);
                let peer = self.peer();
                peer.with_write(WriteOrigin::BlockOps, |d| {
                    let node = d.get_tree(TREE_NAME).create(parent)?;
                    write_stable_id(d, node, &sid(*s))
                })
                .unwrap();
                self.import_from(&peer);
            }
            Transition::ConcurrentDeleteMove {
                victim,
                mover,
                into,
            } => {
                let Some((victim, victim_id)) = pick(&live, victim) else {
                    return;
                };
                if scan[&victim_id][0] != victim {
                    return;
                }
                let peer = self.peer();
                let moved = peer
                    .with_write(WriteOrigin::BlockOps, |d| {
                        let tree = d.get_tree(TREE_NAME);
                        if *into {
                            let candidates: Vec<TreeID> = live
                                .iter()
                                .map(|(n, _)| *n)
                                .filter(|n| !subtree_contains(&tree, *n, victim))
                                .collect();
                            let Some(m) = pick(&candidates, mover) else {
                                return Ok(false);
                            };
                            tree.mov(m, victim)?;
                        } else {
                            let children = tree.children(victim).unwrap_or_default();
                            let Some(m) = pick(&children, mover) else {
                                return Ok(false);
                            };
                            tree.mov(m, None)?;
                        }
                        Ok(true)
                    })
                    .unwrap();
                self.rt
                    .block_on(self.backend.delete_block(&format!("block:{victim_id}")))
                    .unwrap();
                if moved {
                    self.import_from(&peer);
                }
            }
            Transition::LargeImport => {
                self.large_imports += 1;
                let batch = self.large_imports;
                let peer = self.peer();
                peer.with_write(WriteOrigin::BlockOps, |d| {
                    let tree = d.get_tree(TREE_NAME);
                    let root = tree.create(None)?;
                    write_stable_id(d, root, &format!("big{batch}-root"))?;
                    for k in 0..LARGE_IMPORT {
                        let node = tree.create(root)?;
                        write_stable_id(d, node, &format!("big{batch}-{k}"))?;
                    }
                    Ok(())
                })
                .unwrap();
                self.import_from(&peer);
            }
            Transition::HalfBorn { parent } => {
                let parent = parent.as_ref().and_then(|at| pick(&live, at)).map(|p| p.0);
                let node = self
                    .doc
                    .with_write(WriteOrigin::BlockOps, |d| {
                        Ok(d.get_tree(TREE_NAME).create(parent)?)
                    })
                    .unwrap();
                self.half_born.push(node);
            }
            Transition::Settle { node, sid: s } => {
                let alive: Vec<TreeID> = self
                    .doc
                    .with_read(|d| {
                        let tree = d.get_tree(TREE_NAME);
                        Ok(self
                            .half_born
                            .iter()
                            .copied()
                            .filter(|n| matches!(tree.is_node_deleted(n), Ok(false)))
                            .collect())
                    })
                    .unwrap();
                let Some(n) = pick(&alive, node) else {
                    return;
                };
                self.half_born.retain(|h| *h != n);
                let id = self.free_sid(&scan, *s);
                self.doc
                    .with_write(WriteOrigin::BlockOps, |d| write_stable_id(d, n, &id))
                    .unwrap();
            }
            Transition::Batch { ops } => self.batch(ops, &scan),
            Transition::Reload => {
                let doc = Arc::new(
                    LoroDocument::new_with_peer_id("stable-id-pbt".into(), Some(fresh_peer()))
                        .unwrap(),
                );
                doc.apply_update(&self.doc.export_snapshot().unwrap())
                    .unwrap();
                self.backend = LoroBackend::from_document(doc.clone());
                self.doc = doc;
            }
            Transition::Revert { to } => {
                if let Some(frontiers) = pick(&self.history, to) {
                    self.doc
                        .with_write(WriteOrigin::BlockOps, |d| Ok(d.revert_to(&frontiers)?))
                        .unwrap();
                }
            }
        }
        let frontiers = self.doc.with_read(|d| Ok(d.state_frontiers())).unwrap();
        self.history.push(frontiers);
    }

    fn batch(&mut self, ops: &[BatchOp], before: &Scan) {
        let live: Vec<TreeID> = live_nodes(before).into_iter().map(|n| n.0).collect();
        let mut taken = before.clone();
        let mut created: Vec<TreeID> = Vec::new();
        let mut touched: Vec<String> = Vec::new();
        let ids: Vec<String> = ops
            .iter()
            .map(|op| match op {
                BatchOp::Create { sid: s, .. } | BatchOp::Rewrite { sid: s, .. } => {
                    let id = self.free_sid(&taken, *s);
                    taken.insert(id.clone(), Vec::new());
                    id
                }
                BatchOp::Delete { .. } | BatchOp::Revive { .. } | BatchOp::Revert { .. } => {
                    String::new()
                }
            })
            .collect();
        let history = self.history.clone();
        let mut revived = false;
        let doc = self.doc.clone();
        doc.with_write(WriteOrigin::BlockOps, |d| {
            let tree = d.get_tree(TREE_NAME);
            for (op, id) in ops.iter().zip(&ids) {
                let mut candidates = created.clone();
                candidates.extend(live.iter().copied());
                candidates.retain(|n| matches!(tree.is_node_deleted(n), Ok(false)));
                match op {
                    BatchOp::Create { parent, .. } => {
                        let parent = parent.as_ref().and_then(|at| pick(&candidates, at));
                        let node = tree.create(parent)?;
                        write_stable_id(d, node, id)?;
                        created.push(node);
                        touched.push(id.clone());
                    }
                    BatchOp::Rewrite { node, .. } => {
                        let Some(n) = pick(&candidates, node) else {
                            continue;
                        };
                        if let Some(old) = carried_id(&tree, n) {
                            touched.push(old);
                        }
                        write_stable_id(d, n, id)?;
                        touched.push(id.clone());
                    }
                    BatchOp::Delete { node } => {
                        let Some(n) = pick(&candidates, node) else {
                            continue;
                        };
                        if let Some(old) = carried_id(&tree, n) {
                            touched.push(old);
                        }
                        tree.delete(n)?;
                    }
                    BatchOp::Revive { node } => {
                        let Some(n) = pick(&revivable(&tree), node) else {
                            continue;
                        };
                        tree.mov(n, None)?;
                        touched.extend(carried_ids_in_subtree(&tree, n));
                        revived = true;
                    }
                    BatchOp::Revert { to } => {
                        let Some(frontiers) = pick(&history, to) else {
                            continue;
                        };
                        d.revert_to(&frontiers)?;
                        touched.extend(scan_tree(&tree).into_keys());
                    }
                }
                let scan = scan_tree(&tree);
                let mut ids: Vec<String> = touched.clone();
                ids.extend((0..POOL).map(sid));
                if revived {
                    self.check_found(&scan, &ids, "inside a write batch after a revive");
                } else {
                    self.check_lookups(&scan, &ids, "inside a write batch");
                }
            }
            Ok(())
        })
        .unwrap();
    }

    /// I-IDX for `ids`, through the backend's lookup and through the index.
    fn check_lookups(&self, scan: &Scan, ids: &[String], when: &str) {
        for id in ids {
            let expected = scan.get(id).map(|nodes| nodes[0]);
            let backend = self.backend.find_tree_id_by_stable_id_sync(id);
            assert_eq!(
                backend,
                expected,
                "backend lookup of `{id}` {when}: scan holds {:?}",
                scan.get(id)
            );
            let index = self.doc.find_by_stable_id(id).unwrap();
            assert_eq!(
                index,
                expected,
                "index lookup of `{id}` {when}: scan holds {:?}",
                scan.get(id)
            );
        }
    }

    /// A raw move that revives a node is heard at its commit: until then a
    /// lookup finds every revived id, but may name a larger duplicate.
    fn check_found(&self, scan: &Scan, ids: &[String], when: &str) {
        for id in ids {
            let carriers = scan.get(id).cloned().unwrap_or_default();
            let found = [
                self.backend.find_tree_id_by_stable_id_sync(id),
                self.doc.find_by_stable_id(id).unwrap(),
            ];
            for node in found {
                assert!(
                    node.map_or(carriers.is_empty(), |n| carriers.contains(&n)),
                    "lookup of `{id}` {when} answered {node:?}: scan holds {carriers:?}"
                );
            }
        }
    }

    fn check(&self) {
        let scan = self.scan();
        let mut ids: Vec<String> = (0..POOL).map(sid).collect();
        let big: Vec<&String> = scan.keys().filter(|k| k.starts_with("big")).collect();
        let step = (big.len() / SAMPLED).max(1);
        ids.extend(big.into_iter().step_by(step).cloned());
        ids.extend(scan.keys().filter(|k| k.starts_with("fresh")).cloned());
        self.check_lookups(&scan, &ids, "after the transition");
        assert_eq!(
            self.doc.stable_id_index_snapshot().unwrap(),
            scan,
            "the index's live set differs from the full scan"
        );
    }
}

/// Deleted nodes whose parent is a deleted node, not the deleted marker: a
/// move to the root revives them.
fn revivable(tree: &LoroTree) -> Vec<TreeID> {
    tree.get_nodes(true)
        .into_iter()
        .filter(|n| matches!(n.parent, TreeParentId::Node(_)))
        .filter(|n| matches!(tree.is_node_deleted(&n.id), Ok(true)))
        .map(|n| n.id)
        .collect()
}

fn carried_ids_in_subtree(tree: &LoroTree, root: TreeID) -> Vec<String> {
    let mut ids = Vec::new();
    let mut queue = vec![root];
    while let Some(node) = queue.pop() {
        ids.extend(carried_id(tree, node));
        queue.extend(tree.children(node).unwrap_or_default());
    }
    ids
}

fn carried_id(tree: &LoroTree, node: TreeID) -> Option<String> {
    let value = tree
        .get_meta(node)
        .ok()?
        .get(STABLE_ID)?
        .into_value()
        .ok()?;
    Some(value.into_string().ok()?.to_string())
}

struct StableIdIndexTest;

impl StateMachineTest for StableIdIndexTest {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &Ref) -> Sut {
        Sut::new()
    }

    fn apply(mut sut: Sut, _: &Ref, transition: Transition) -> Sut {
        sut.apply(&transition);
        sut
    }

    fn check_invariants(sut: &Sut, _: &Ref) {
        sut.check();
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        max_shrink_iters: 2000,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn stable_id_index_matches_the_full_scan(sequential 1..40 => StableIdIndexTest);
}

/// The shapes the state machine must reach, pinned so each one runs on every
/// run instead of when the generator happens to draw it.
fn replay(transitions: &[Transition]) {
    let mut sut = Sut::new();
    for transition in transitions {
        sut.apply(transition);
        sut.check();
    }
}

#[test]
fn an_imported_create_is_found() {
    replay(&[Transition::PeerCreate {
        sid: 0,
        parent: None,
    }]);
}

#[test]
fn a_parent_created_in_the_same_batch_is_found() {
    replay(&[Transition::CreateChain {
        sids: vec![0, 1, 2],
        parent: None,
    }]);
}

#[test]
fn a_large_import_is_found() {
    replay(&[
        Transition::Create {
            sid: 0,
            parent: None,
        },
        Transition::LargeImport,
    ]);
}

#[test]
fn a_duplicated_id_answers_its_smallest_carrier() {
    replay(&[
        Transition::Create {
            sid: 0,
            parent: None,
        },
        Transition::PeerCreate {
            sid: 0,
            parent: None,
        },
    ]);
}

#[test]
fn a_node_revived_inside_a_batch_is_found() {
    let sut = Sut::new();
    let create = |parent: EntityUri, id: &str| {
        sut.rt
            .block_on(sut.backend.create_block(
                parent,
                BlockContent::text("x"),
                Some(EntityUri::block(id)),
            ))
            .unwrap();
    };
    create(EntityUri::no_parent(), "p");
    create(EntityUri::block("p"), "c");
    let c = sut.doc.find_by_stable_id("c").unwrap().unwrap();
    sut.rt
        .block_on(sut.backend.delete_block("block:p"))
        .unwrap();
    sut.check();
    sut.doc
        .with_write(WriteOrigin::BlockOps, |d| {
            d.get_tree(TREE_NAME).mov(c, None)?;
            let scan = scan_tree(&d.get_tree(TREE_NAME));
            sut.check_lookups(&scan, &["c".to_string()], "after a revive inside the batch");
            Ok(())
        })
        .unwrap();
    sut.check();
}

#[test]
fn a_node_reverted_inside_a_batch_is_found() {
    let sut = Sut::new();
    let create = |id: &str| {
        sut.rt
            .block_on(sut.backend.create_block(
                EntityUri::no_parent(),
                BlockContent::text("x"),
                Some(EntityUri::block(id)),
            ))
            .unwrap();
    };
    create("a");
    let before_b = sut.doc.with_read(|d| Ok(d.state_frontiers())).unwrap();
    create("b");
    sut.rt
        .block_on(sut.backend.delete_block("block:a"))
        .unwrap();
    sut.doc
        .with_write(WriteOrigin::BlockOps, |d| {
            d.revert_to(&before_b)?;
            let scan = scan_tree(&d.get_tree(TREE_NAME));
            let ids = ["a".to_string(), "b".to_string()];
            sut.check_lookups(&scan, &ids, "after a revert inside the batch");
            Ok(())
        })
        .unwrap();
    sut.check();
}

#[test]
fn a_duplicate_reverted_inside_a_batch_answers_its_smallest_carrier() {
    let sut = Sut::new();
    sut.rt
        .block_on(sut.backend.create_block(
            EntityUri::no_parent(),
            BlockContent::text("x"),
            Some(EntityUri::block("a")),
        ))
        .unwrap();
    let peer = sut.peer();
    peer.with_write(WriteOrigin::BlockOps, |d| {
        let node = d.get_tree(TREE_NAME).create(None)?;
        write_stable_id(d, node, "a")
    })
    .unwrap();
    sut.import_from(&peer);
    let both = sut.doc.with_read(|d| Ok(d.state_frontiers())).unwrap();
    sut.rt
        .block_on(sut.backend.delete_block("block:a"))
        .unwrap();
    sut.check();
    sut.doc
        .with_write(WriteOrigin::BlockOps, |d| {
            d.revert_to(&both)?;
            let scan = scan_tree(&d.get_tree(TREE_NAME));
            assert_eq!(
                scan["a"].len(),
                2,
                "the revert brings the deleted carrier back"
            );
            sut.check_lookups(&scan, &["a".to_string()], "after a revert inside the batch");
            Ok(())
        })
        .unwrap();
    sut.check();
}
