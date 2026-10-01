//! Probe: does every change that can alter the `stable id -> TreeID` map reach
//! a subscription on the block tree container?
//!
//! The planned stable-id index (deep-vault plan, D1.a) treats a lookup miss as
//! authoritative, so a change the subscription does not hear is a false miss
//! and a false miss is a duplicate node. The index re-reads the CURRENT state
//! of each touched node and its subtree, so the facts it needs are:
//!
//! - a create, move or delete reports its target node (a tree diff);
//! - a write of the `id` meta key on an EXISTING node reports that node (a map
//!   diff whose path names it), because no tree diff accompanies it;
//! - both arrive on the writing thread before the write call returns.
//!
//! Each test names the event source it measures and states its verdict.

use std::sync::Arc;
use std::sync::Mutex;

use loro::EventTriggerKind;
use loro::Index;
use loro::LoroDoc;
use loro::LoroTree;
use loro::TreeExternalDiff;
use loro::TreeID;
use loro::event::Diff;

use crate::LoroDocument;
use crate::WriteOrigin;
use crate::loro_backend::STABLE_ID;
use crate::loro_backend::TREE_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Create,
    Move,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Tree {
        by: EventTriggerKind,
        node: TreeID,
        action: Action,
    },
    /// A map diff below the tree; `node` is the `Index::Node` its path ends in.
    Meta {
        by: EventTriggerKind,
        node: TreeID,
        keys: Vec<String>,
    },
    /// A diff below the tree whose path names no node.
    Unplaced { by: EventTriggerKind, path: String },
}

struct Recorder {
    seen: Arc<Mutex<Vec<Seen>>>,
    _subscription: loro::Subscription,
}

impl Recorder {
    fn on(doc: &LoroDocument) -> Self {
        // ALLOW(loro_doc_escape): a subscription registration; the callback reads only
        // its event.
        let doc = doc.doc();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let tree = doc.get_tree(TREE_NAME);
        let subscription = doc.subscribe(
            &loro::ContainerTrait::id(&tree),
            Arc::new(move |event| {
                let mut sink = sink.lock().unwrap();
                for diff in &event.events {
                    let by = event.triggered_by;
                    match &diff.diff {
                        Diff::Tree(tree_diff) => {
                            sink.extend(tree_diff.diff.iter().map(|item| Seen::Tree {
                                by,
                                node: item.target,
                                action: match item.action {
                                    TreeExternalDiff::Create { .. } => Action::Create,
                                    TreeExternalDiff::Move { .. } => Action::Move,
                                    TreeExternalDiff::Delete { .. } => Action::Delete,
                                },
                            }));
                        }
                        Diff::Map(map) => {
                            let node = diff.path.iter().rev().find_map(|(_, index)| match index {
                                Index::Node(node) => Some(*node),
                                _ => None,
                            });
                            let mut keys: Vec<String> =
                                map.updated.keys().map(|key| key.to_string()).collect();
                            keys.sort();
                            sink.push(match node {
                                Some(node) => Seen::Meta { by, node, keys },
                                None => Seen::Unplaced {
                                    by,
                                    path: format!("{:?}", diff.path),
                                },
                            });
                        }
                        _ => {}
                    }
                }
            }),
        );
        Self {
            seen,
            _subscription: subscription,
        }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

fn tree_of(doc: &LoroDocument) -> LoroTree {
    // ALLOW(loro_doc_escape): measurement probe — container handles only; every
    // mutation goes through `with_write`.
    doc.doc().get_tree(TREE_NAME)
}

fn create_with_id(tree: &LoroTree, parent: Option<TreeID>, sid: &str) -> TreeID {
    let node = tree.create(parent).unwrap();
    tree.get_meta(node)
        .unwrap()
        .insert(STABLE_ID, loro::LoroValue::from(sid))
        .unwrap();
    node
}

fn tree_seen(seen: &[Seen], by: EventTriggerKind, node: TreeID, action: Action) -> bool {
    seen.contains(&Seen::Tree { by, node, action })
}

fn id_key_seen(seen: &[Seen], by: EventTriggerKind, node: TreeID) -> bool {
    seen.iter().any(|s| {
        matches!(s, Seen::Meta { by: b, node: n, keys } if *b == by && *n == node && keys.iter().any(|k| k == STABLE_ID))
    })
}

fn no_unplaced(seen: &[Seen]) {
    let unplaced: Vec<_> = seen
        .iter()
        .filter(|s| matches!(s, Seen::Unplaced { .. }))
        .collect();
    assert!(
        unplaced.is_empty(),
        "map diffs with no node in their path: {unplaced:?}"
    );
}

fn new_doc(name: &str, peer: u64) -> LoroDocument {
    LoroDocument::new_with_peer_id(name.to_string(), Some(peer)).unwrap()
}

/// VERDICT 1 — a local `with_write` commit delivers the create AND the `id`
/// key of a node created in the same batch, and the map diff's path names the
/// node. Both arrive before `with_write` returns.
#[test]
fn local_commit_delivers_create_and_id_key_before_return() {
    let doc = new_doc("probe-local", 1);
    let recorder = Recorder::on(&doc);
    let tree = tree_of(&doc);
    let parent = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            Ok(create_with_id(&tree, None, "p"))
        })
        .unwrap();
    let seen = recorder.take();
    assert!(
        tree_seen(&seen, EventTriggerKind::Local, parent, Action::Create),
        "{seen:?}"
    );
    assert!(
        id_key_seen(&seen, EventTriggerKind::Local, parent),
        "{seen:?}"
    );
    no_unplaced(&seen);

    let child = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            Ok(create_with_id(&tree, Some(parent), "c"))
        })
        .unwrap();
    let seen = recorder.take();
    assert!(
        tree_seen(&seen, EventTriggerKind::Local, child, Action::Create),
        "{seen:?}"
    );
    assert!(
        id_key_seen(&seen, EventTriggerKind::Local, child),
        "{seen:?}"
    );
    no_unplaced(&seen);
}

/// VERDICT 2 — `FlushOnDrop`: a batch whose closure fails still commits, and
/// its create and `id` key are delivered before `with_write` returns the
/// error.
#[test]
fn failed_batch_flush_delivers_its_create_and_id_key() {
    let doc = new_doc("probe-flush", 1);
    let recorder = Recorder::on(&doc);
    let tree = tree_of(&doc);
    let made = Mutex::new(None);
    let err = doc
        .with_write(WriteOrigin::BlockOps, |_| -> anyhow::Result<()> {
            *made.lock().unwrap() = Some(create_with_id(&tree, None, "half"));
            anyhow::bail!("op 2 fails")
        })
        .unwrap_err();
    assert!(err.to_string().contains("op 2 fails"));
    let node = made.lock().unwrap().unwrap();
    let seen = recorder.take();
    assert!(
        tree_seen(&seen, EventTriggerKind::Local, node, Action::Create),
        "{seen:?}"
    );
    assert!(
        id_key_seen(&seen, EventTriggerKind::Local, node),
        "{seen:?}"
    );
}

/// VERDICT 3 — an `id` rewrite on an existing node (`set_external_id`) and a
/// half-born node (created without an id, id in a later commit) both deliver
/// a map diff naming the node. A move delivers the moved node; a subtree
/// delete reports ONLY the subtree root, so the index must walk the subtree.
#[test]
fn id_rewrite_move_and_subtree_delete_report_their_node() {
    let doc = new_doc("probe-meta", 1);
    let recorder = Recorder::on(&doc);
    let tree = tree_of(&doc);
    let (a, b, c) = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            let a = create_with_id(&tree, None, "a");
            let b = create_with_id(&tree, None, "b");
            let c = create_with_id(&tree, Some(b), "c");
            Ok((a, b, c))
        })
        .unwrap();
    recorder.take();

    doc.with_write(WriteOrigin::BlockOps, |_| {
        tree.get_meta(a).unwrap().insert(STABLE_ID, "a2")?;
        Ok(())
    })
    .unwrap();
    let seen = recorder.take();
    assert!(id_key_seen(&seen, EventTriggerKind::Local, a), "{seen:?}");

    let born = doc
        .with_write(WriteOrigin::BlockOps, |_| Ok(tree.create(None)?))
        .unwrap();
    assert!(tree_seen(
        &recorder.take(),
        EventTriggerKind::Local,
        born,
        Action::Create
    ));
    doc.with_write(WriteOrigin::BlockOps, |_| {
        tree.get_meta(born).unwrap().insert(STABLE_ID, "late")?;
        Ok(())
    })
    .unwrap();
    let seen = recorder.take();
    assert!(
        id_key_seen(&seen, EventTriggerKind::Local, born),
        "{seen:?}"
    );

    doc.with_write(WriteOrigin::BlockOps, |_| Ok(tree.mov(b, Some(a))?))
        .unwrap();
    let seen = recorder.take();
    assert!(
        tree_seen(&seen, EventTriggerKind::Local, b, Action::Move),
        "{seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|s| matches!(s, Seen::Tree { node, .. } if *node == c)),
        "{seen:?}"
    );

    doc.with_write(WriteOrigin::BlockOps, |_| Ok(tree.delete(a)?))
        .unwrap();
    let seen = recorder.take();
    assert!(
        tree_seen(&seen, EventTriggerKind::Local, a, Action::Delete),
        "{seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|s| matches!(s, Seen::Tree { node, .. } if *node == b || *node == c)),
        "a subtree delete reported a descendant: {seen:?}"
    );
}

/// Peer B's ops: one node created with its id in one commit, one half-born
/// node whose id arrives in a later commit, and an id rewrite of a node A
/// already knows.
fn peer_b_ops(base: &[u8]) -> (Vec<u8>, TreeID, TreeID, TreeID) {
    let b = LoroDoc::new();
    b.set_peer_id(2).unwrap();
    b.import(base).unwrap();
    let start = b.oplog_vv();
    let tree = b.get_tree(TREE_NAME);
    let shared = tree
        .get_nodes(false)
        .into_iter()
        .find(|n| {
            !matches!(
                n.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            )
        })
        .unwrap()
        .id;
    let whole = create_with_id(&tree, None, "from-b");
    b.commit();
    let half = tree.create(None).unwrap();
    b.commit();
    tree.get_meta(half)
        .unwrap()
        .insert(STABLE_ID, "b-late")
        .unwrap();
    tree.get_meta(shared)
        .unwrap()
        .insert(STABLE_ID, "rewritten-by-b")
        .unwrap();
    b.commit();
    let update = b.export(loro::ExportMode::updates(&start)).unwrap();
    (update, whole, half, shared)
}

fn seeded(name: &str) -> (LoroDocument, Vec<u8>) {
    let doc = new_doc(name, 1);
    let tree = tree_of(&doc);
    doc.with_write(WriteOrigin::BlockOps, |_| {
        Ok(create_with_id(&tree, None, "seed"))
    })
    .unwrap();
    let snapshot = doc.export_snapshot().unwrap();
    (doc, snapshot)
}

fn assert_peer_b_delivered(seen: &[Seen], whole: TreeID, half: TreeID, shared: TreeID) {
    let by = EventTriggerKind::Import;
    assert!(tree_seen(seen, by, whole, Action::Create), "{seen:?}");
    assert!(tree_seen(seen, by, half, Action::Create), "{seen:?}");
    assert!(
        id_key_seen(seen, by, shared),
        "id rewrite of a known node: {seen:?}"
    );
    no_unplaced(seen);
}

/// VERDICT 4 — an import through `apply_update_with_origin` (reconcile,
/// pairing, peer delta) delivers every created node and the `id` rewrite of a
/// node the importer already had.
#[test]
fn import_delivers_remote_creates_and_id_rewrites() {
    let (doc, snapshot) = seeded("probe-import");
    let recorder = Recorder::on(&doc);
    let (update, whole, half, shared) = peer_b_ops(&snapshot);
    doc.apply_update_with_origin(WriteOrigin::SyncImport, &update)
        .unwrap();
    let seen = recorder.take();
    assert_peer_b_delivered(&seen, whole, half, shared);
}

/// VERDICT 5 — the same import through `WriteTxn::import` inside a
/// `with_write` batch is delivered before `with_write` returns.
#[test]
fn import_inside_a_write_batch_is_delivered() {
    let (doc, snapshot) = seeded("probe-txn-import");
    let recorder = Recorder::on(&doc);
    let (update, whole, half, shared) = peer_b_ops(&snapshot);
    doc.with_write(WriteOrigin::SyncImport, |txn| txn.import(&update))
        .unwrap();
    let seen = recorder.take();
    assert_peer_b_delivered(&seen, whole, half, shared);
}

/// VERDICT 6 — a large import (past `MountIndex`'s 1024 burst limit) reports
/// every created node.
#[test]
fn large_import_reports_every_create() {
    let (doc, snapshot) = seeded("probe-large");
    let recorder = Recorder::on(&doc);
    let b = LoroDoc::new();
    b.set_peer_id(2).unwrap();
    b.import(&snapshot).unwrap();
    let start = b.oplog_vv();
    let tree = b.get_tree(TREE_NAME);
    let made: Vec<TreeID> = (0..1500)
        .map(|i| create_with_id(&tree, None, &format!("bulk-{i}")))
        .collect();
    b.commit();
    doc.apply_update_with_origin(
        WriteOrigin::SyncImport,
        &b.export(loro::ExportMode::updates(&start)).unwrap(),
    )
    .unwrap();
    let seen = recorder.take();
    let missing: Vec<_> = made
        .iter()
        .filter(|&&n| !tree_seen(&seen, EventTriggerKind::Import, n, Action::Create))
        .collect();
    assert!(
        missing.is_empty(),
        "{} of 1500 creates not reported",
        missing.len()
    );
}

/// VERDICT 7 — `checkout` to a version before a create and an `id` rewrite
/// reports the node's removal and the rewrite's undo; `checkout_to_latest`
/// reports both again.
#[test]
fn checkout_reports_creates_deletes_and_id_rewrites() {
    let doc = new_doc("probe-checkout", 1);
    let tree = tree_of(&doc);
    let a = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            Ok(create_with_id(&tree, None, "a"))
        })
        .unwrap();
    // ALLOW(loro_doc_escape): measurement probe — `checkout` is not exposed
    // through the doc-boundary API.
    let raw = doc.doc();
    let before = raw.state_frontiers();
    let later = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            tree.get_meta(a).unwrap().insert(STABLE_ID, "a2")?;
            Ok(create_with_id(&tree, None, "later"))
        })
        .unwrap();
    let recorder = Recorder::on(&doc);

    raw.checkout(&before).unwrap();
    let seen = recorder.take();
    let by = EventTriggerKind::Checkout;
    assert!(tree_seen(&seen, by, later, Action::Delete), "{seen:?}");
    assert!(id_key_seen(&seen, by, a), "{seen:?}");
    no_unplaced(&seen);

    raw.checkout_to_latest();
    let seen = recorder.take();
    assert!(tree_seen(&seen, by, later, Action::Create), "{seen:?}");
    assert!(id_key_seen(&seen, by, a), "{seen:?}");
    no_unplaced(&seen);
}

/// VERDICT 8 — `revert_to` (D146.a) is a local commit: it reports the
/// removal of a node created after the target version and the undo of an
/// `id` rewrite.
#[test]
fn revert_to_reports_creates_and_id_rewrites_it_undoes() {
    let doc = new_doc("probe-revert", 1);
    let tree = tree_of(&doc);
    let a = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            Ok(create_with_id(&tree, None, "a"))
        })
        .unwrap();
    // ALLOW(loro_doc_escape): measurement probe — `revert_to` is not exposed
    // through the doc-boundary API.
    let raw = doc.doc();
    let before = raw.state_frontiers();
    let later = doc
        .with_write(WriteOrigin::BlockOps, |_| {
            tree.get_meta(a).unwrap().insert(STABLE_ID, "a2")?;
            Ok(create_with_id(&tree, None, "later"))
        })
        .unwrap();
    let recorder = Recorder::on(&doc);

    doc.with_write(WriteOrigin::BlockOps, |txn| Ok(txn.revert_to(&before)?))
        .unwrap();
    let seen = recorder.take();
    let by = EventTriggerKind::Local;
    assert!(tree_seen(&seen, by, later, Action::Delete), "{seen:?}");
    assert!(id_key_seen(&seen, by, a), "{seen:?}");
    no_unplaced(&seen);
}
