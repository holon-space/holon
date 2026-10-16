//! Step 0 of the guarded-slot convergence spike (#351, concept #345): the four
//! Loro facts the `Resolve<K>` design rests on, measured rather than assumed.
//!
//! | verdict | question |
//! |---|---|
//! | (a) | two devices mint one `STABLE_ID` concurrently — what does a sync give? |
//! | (b) | do concurrent moves of one child to one parent converge, position included? |
//! | (c) | does delete + undo re-create the node with a NEW op id? |
//! | (d) | are block ids random 128-bit values, never content-derived? |
//!
//! Spike, test only: nothing here is production code.

use std::collections::BTreeSet;

use holon_api::identity_minting::MintedId;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroTree;
use loro::LoroValue;
use loro::TreeID;
use loro::TreeParentId;
use loro::UndoManager;

/// The duplicated well-known key both devices claim.
const KEY: &str = "journals";
/// holon-loro's `STABLE_ID` meta key and tree name (`loro_backend.rs`): the
/// block id is meta, the `TreeID` is peer-local (ADR 0003).
const STABLE_ID: &str = "id";
const TREE: &str = "blocks";

fn device(peer: u64) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer).expect("a fresh doc takes a peer id");
    doc.get_tree(TREE).enable_fractional_index(0);
    doc
}

/// One sync round in both directions, to a fixed point in one pass: an export
/// against the peer's version vector carries exactly what the peer is missing.
fn sync(a: &LoroDoc, b: &LoroDoc) {
    let to_b = a
        .export(ExportMode::updates(&b.oplog_vv()))
        .expect("the export succeeds");
    let to_a = b
        .export(ExportMode::updates(&a.oplog_vv()))
        .expect("the export succeeds");
    b.import(&to_b).expect("the import succeeds");
    a.import(&to_a).expect("the import succeeds");
}

/// Creates a node under the root carrying `STABLE_ID = id` plus a text field,
/// the shape a block takes in the tree.
fn claim(doc: &LoroDoc, id: &str, text: &str) -> TreeID {
    let tree = doc.get_tree(TREE);
    let node = tree
        .create(TreeParentId::Root)
        .expect("the root accepts a child");
    let meta = tree.get_meta(node).expect("a fresh node has meta");
    meta.insert(STABLE_ID, id).expect("meta takes the id");
    meta.insert("text", text).expect("meta takes the text");
    doc.commit();
    node
}

fn child(doc: &LoroDoc, parent: TreeID, id: &str) -> TreeID {
    let tree = doc.get_tree(TREE);
    let node = tree
        .create(TreeParentId::Node(parent))
        .expect("the parent accepts a child");
    tree.get_meta(node)
        .expect("a fresh node has meta")
        .insert(STABLE_ID, id)
        .expect("meta takes the id");
    doc.commit();
    node
}

fn meta_str(tree: &LoroTree, node: TreeID, field: &str) -> Option<String> {
    match tree.get_meta(node).ok()?.get(field) {
        Some(loro::ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

/// Every live node of the tree that carries `STABLE_ID == id`, in `TreeID`
/// order — the contestant set a device reads out of its own synced doc.
fn carriers(doc: &LoroDoc, id: &str) -> Vec<TreeID> {
    let tree = doc.get_tree(TREE);
    let mut found: Vec<TreeID> = tree
        .nodes()
        .into_iter()
        .filter(|n| !tree.is_node_deleted(n).unwrap_or(true))
        .filter(|n| meta_str(&tree, *n, STABLE_ID).as_deref() == Some(id))
        .collect();
    found.sort();
    found
}

fn children_of(doc: &LoroDoc, parent: TreeID) -> Vec<TreeID> {
    doc.get_tree(TREE)
        .children(TreeParentId::Node(parent))
        .unwrap_or_default()
}

/// The fractional index of a node as a comparable string, so two devices can be
/// compared on the POSITION and not only on the parent.
fn position(doc: &LoroDoc, node: TreeID) -> String {
    format!(
        "{:?}",
        doc.get_tree(TREE)
            .fractional_index(node)
            .expect("a live node has a fractional index")
    )
}

// ---------------------------------------------------------------- verdict (a)

/// Two devices each create a node carrying `STABLE_ID = "journals"`, each adds
/// children and edits its own fields, then they sync.
///
/// The measurement: the sync does NOT merge the two nodes. Both devices end up
/// with TWO live carriers of one id, each keeping its own children and fields,
/// and both devices see the SAME two `TreeID`s in the same order. That is the
/// whole justification for `Resolve<K>`: Loro gives convergence of the
/// contestant SET, never a decision about which carrier holds the slot.
#[test]
fn a_two_devices_minting_one_stable_id_converge_to_two_carriers_not_one() {
    let (a, b) = (device(1), device(2));

    let from_a = claim(&a, KEY, "from-a");
    child(&a, from_a, "a-kid-1");
    child(&a, from_a, "a-kid-2");

    let from_b = claim(&b, KEY, "from-b");
    child(&b, from_b, "b-kid-1");

    sync(&a, &b);

    assert_eq!(
        carriers(&a, KEY),
        carriers(&b, KEY),
        "(a) the two devices disagree about the contestant set"
    );
    let set = carriers(&a, KEY);
    assert_eq!(
        set.len(),
        2,
        "(a) Loro merged the two carriers by itself: {set:?}"
    );
    assert_eq!(
        set,
        {
            let mut both = vec![from_a, from_b];
            both.sort();
            both
        },
        "(a) the contestant set is not the two created nodes"
    );

    // Each carrier kept its own children and its own field value.
    for doc in [&a, &b] {
        assert_eq!(children_of(doc, from_a).len(), 2, "(a) A lost children");
        assert_eq!(children_of(doc, from_b).len(), 1, "(a) B lost children");
        let tree = doc.get_tree(TREE);
        assert_eq!(meta_str(&tree, from_a, "text").as_deref(), Some("from-a"));
        assert_eq!(meta_str(&tree, from_b, "text").as_deref(), Some("from-b"));
    }

    // An edit on one side after the sync reaches the other and changes nothing
    // about the SET, which is what lets `Resolve` fire on set changes only.
    a.get_tree(TREE)
        .get_meta(from_a)
        .expect("meta")
        .insert("text", "edited-on-a")
        .expect("meta takes the text");
    a.commit();
    sync(&a, &b);
    assert_eq!(
        carriers(&b, KEY),
        set,
        "(a) an edit of a contestant changed the contestant set"
    );
    assert_eq!(
        meta_str(&b.get_tree(TREE), from_a, "text").as_deref(),
        Some("edited-on-a"),
        "(a) the edit did not reach the other device"
    );
}

// ---------------------------------------------------------------- verdict (b)

/// Both devices move the SAME child under the SAME new parent, each choosing a
/// different fractional position among that parent's existing children.
///
/// The measurement: after the sync the child is one node under one parent on
/// both devices, at ONE position both agree on. A concurrent move is therefore
/// safe for the `merge` outcome (#345: children merge by MOVE, never by copy) —
/// a device that resolves locally and a device that resolves later cannot
/// duplicate the child.
#[test]
fn b_concurrent_moves_of_one_child_to_one_parent_converge_with_the_position() {
    let (a, b) = (device(1), device(2));

    let main = claim(&a, KEY, "main");
    let m1 = child(&a, main, "m1");
    let m2 = child(&a, main, "m2");
    let secondary = claim(&a, "other", "secondary");
    let moved = child(&a, secondary, "moved");
    sync(&a, &b);
    assert_eq!(
        children_of(&b, main),
        vec![m1, m2],
        "(b) the sync is unequal"
    );

    // A puts the child first, B puts it last: two moves, same node, same new
    // parent, different fractional index.
    a.get_tree(TREE)
        .mov_to(moved, TreeParentId::Node(main), 0)
        .expect("A's move lands");
    a.commit();
    b.get_tree(TREE)
        .mov_to(moved, TreeParentId::Node(main), 2)
        .expect("B's move lands");
    b.commit();

    sync(&a, &b);

    let kids_a = children_of(&a, main);
    let kids_b = children_of(&b, main);
    assert_eq!(kids_a, kids_b, "(b) the two devices disagree on the order");
    assert_eq!(
        kids_a.iter().collect::<BTreeSet<_>>().len(),
        kids_a.len(),
        "(b) a child appears twice: {kids_a:?}"
    );
    assert_eq!(
        kids_a.iter().filter(|&&n| n == moved).count(),
        1,
        "(b) the moved child is not under its new parent exactly once: {kids_a:?}"
    );
    assert!(
        children_of(&a, secondary).is_empty(),
        "(b) the child is still under its old parent as well"
    );
    assert_eq!(
        position(&a, moved),
        position(&b, moved),
        "(b) the devices agree on the order but not on the fractional index"
    );
}

// ---------------------------------------------------------------- verdict (c)

/// Delete a node, then undo the delete.
///
/// The measurement decides whether the winner order may be derived from the
/// node's current op id. `TreeID` IS the id of the creating op, so if undo
/// re-creates the node the id changes and a derived order would flip on an
/// undo — which is why #345 ruled a STORED claim stamp.
#[test]
fn c_delete_then_undo_tells_whether_the_op_id_is_new() {
    let doc = device(1);
    let mut undo = UndoManager::new(&doc);

    let node = claim(&doc, KEY, "only");
    let before = node;

    doc.get_tree(TREE).delete(node).expect("the delete lands");
    doc.commit();
    assert!(
        doc.get_tree(TREE)
            .is_node_deleted(&node)
            .expect("the node is known"),
        "(c) the delete did not take"
    );

    assert!(undo.can_undo(), "(c) the manager did not see the delete");
    undo.undo().expect("the undo lands");
    doc.commit();

    let after = carriers(&doc, KEY);
    assert_eq!(
        after.len(),
        1,
        "(c) the undo did not restore exactly one carrier: {after:?}"
    );
    let restored = after[0];
    // VERDICT (c): the undo re-creates the node, so its `TreeID` is the id of
    // the NEW op. This is why #345 ruled a STORED claim stamp: a preference
    // term read off the current op id would flip on an undo.
    assert_ne!(
        restored, before,
        "(c) undo REUSED the TreeID ({restored:?}): the stored-stamp ruling of #345 \
         rests on it being new, so a reuse changes the design's premise"
    );
    assert_eq!(
        meta_str(&doc.get_tree(TREE), restored, "text").as_deref(),
        Some("only"),
        "(c) the undo lost the node's fields"
    );
}

/// The same question for the shape the design actually meets: the node is
/// deleted on one device and the delete is undone on the OTHER after a sync.
#[test]
fn c_delete_on_one_device_undone_on_the_other_restores_one_carrier() {
    let (a, b) = (device(1), device(2));
    let node = claim(&a, KEY, "only");
    sync(&a, &b);

    let mut undo_b = UndoManager::new(&b);
    // B must own an op on the node for its UndoManager to hold an inverse:
    // an UndoManager undoes its own peer's local ops only.
    b.get_tree(TREE)
        .get_meta(node)
        .expect("meta")
        .insert("text", "touched-on-b")
        .expect("meta takes the text");
    b.commit();

    a.get_tree(TREE).delete(node).expect("the delete lands");
    a.commit();
    sync(&a, &b);
    assert!(
        b.get_tree(TREE)
            .is_node_deleted(&node)
            .expect("the node is known"),
        "(c) the remote delete did not reach B"
    );

    assert!(undo_b.can_undo(), "(c) B has nothing to undo");
    undo_b.undo().expect("the undo lands");
    b.commit();
    sync(&a, &b);

    assert_eq!(
        carriers(&a, KEY),
        carriers(&b, KEY),
        "(c) the devices disagree after a cross-device delete and undo"
    );
}

// ---------------------------------------------------------------- verdict (d)

/// Block ids come from `MintedId::random()` — "the single sanctioned fresh-id
/// mint" (ADR 0029), which is `EntityUri::block_random()`, which is
/// `Uuid::new_v4` (`crates/holon-api/src/entity_uri.rs:154`). This pins the two
/// properties the design needs: the value is random, and it is not a function
/// of anything the block contains, so an edit can never change an id and two
/// different entities can only share an id by being copied.
#[test]
fn d_block_ids_are_random_never_content_derived() {
    const N: usize = 4096;
    let ids: BTreeSet<String> = (0..N)
        .map(|_| MintedId::random().as_str().to_string())
        .collect();
    assert_eq!(ids.len(), N, "(d) a block id repeated inside {N} mints");

    for id in ids.iter().take(8) {
        let raw = id
            .strip_prefix("block:")
            .expect("(d) a minted block id carries the block scheme");
        let bytes = uuid::Uuid::parse_str(raw).expect("(d) a block id is a UUID");
        assert_eq!(bytes.get_version_num(), 4, "(d) a block id is not UUID v4");
    }

    // Content-derived would mean: equal content gives an equal id. Two mints for
    // the same conceptual block do not.
    assert_ne!(
        MintedId::random().as_str(),
        MintedId::random().as_str(),
        "(d) two mints agreed, so the id is derived from something, not random"
    );
}

/// (e) A fifth fact, found by the proptest of this spike rather than asked for
/// up front: a sub-container reached by "get the field, or create it when it is
/// absent" is NOT a convergent union.
///
/// Both devices hold one synced node whose `tags` field does not exist yet, and
/// each writes its first tag. Each `insert_container` is a write of the map KEY
/// `tags`, so the two containers meet under last-write-wins: one survives whole
/// and the other's entries are unreachable on BOTH devices. The tag is not
/// merged, it is gone.
///
/// The consequence for #345: "tags union" and "LOGBOOK append" converge only
/// when the container is created ONCE, by the op that creates the node. A
/// lazily created one silently drops a concurrent first write.
#[test]
fn e_a_lazily_created_sub_container_loses_a_concurrent_first_write() {
    let (a, b) = (device(1), device(2));
    let node = claim(&a, KEY, "shared");
    sync(&a, &b);

    // Neither side has a `tags` container yet, so each creates its own.
    for (doc, tag) in [(&a, "from-a"), (&b, "from-b")] {
        let meta = doc
            .get_tree(TREE)
            .get_meta(node)
            .expect("the synced node has meta");
        let tags = meta
            .insert_container("tags", loro::LoroMap::new())
            .expect("meta takes a map container");
        tags.insert(tag, true).expect("the set takes the tag");
        doc.commit();
    }
    sync(&a, &b);

    let surviving = |doc: &LoroDoc| -> BTreeSet<String> {
        let meta = doc
            .get_tree(TREE)
            .get_meta(node)
            .expect("the node has meta");
        match meta.get("tags") {
            Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => {
                m.keys().map(|k| k.to_string()).collect()
            }
            _ => BTreeSet::new(),
        }
    };

    // Convergent: both devices agree on WHICH container won.
    assert_eq!(
        surviving(&a),
        surviving(&b),
        "(e) the devices disagree on the surviving tag container"
    );
    // ...but the union was lost. One tag only, not two.
    assert_eq!(
        surviving(&a).len(),
        1,
        "(e) a lazily created container kept both concurrent tags ({:?}): the \
         eager-creation requirement this pins is no longer needed",
        surviving(&a)
    );
}
