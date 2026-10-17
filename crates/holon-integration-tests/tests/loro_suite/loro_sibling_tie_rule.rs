//! Loro's tie rule for two siblings created concurrently at the SAME
//! fractional index: the order key is `NodePosition { position, idlp }` with a
//! derived `Ord`, so after the position ties it is `(lamport, peer)` of the
//! node's last move op that decides. Two consequences this pins:
//!
//! - equal Lamport: the LOWER peer id comes first;
//! - different Lamport: the HIGHER Lamport comes LATER, whichever peer holds it
//!   — so the peer id only ever breaks an equal-Lamport tie.
//!
//! Holon's sibling order is user-visible, so a Loro upgrade that changed this
//! rule would silently reorder blocks. This test runs raw Loro docs (no Holon
//! layer) and is a member of the landing gate's `loro-suite` step, so such an
//! upgrade goes red at once.
//!
//! @pbt kind example
//! @pbt covers loro-sibling-tie-rule — concurrent same-position siblings order
//! by (lamport, peer) of their create op

use loro::LoroDoc;
use loro::TreeID;
use loro::TreeParentId;

const TREE: &str = "tie_rule_tree";

/// How many extra single-atom commits a peer makes before it creates its
/// child, which raises that child's create Lamport by the same amount.
const PAD: usize = 5;

/// Two peers create one child each under a shared parent, at the same
/// fractional index. `pad_on` names the peer that first makes [`PAD`] extra
/// commits, so its create op carries the higher Lamport; `None` leaves the two
/// create ops on the same Lamport.
///
/// Returns the peer id of each child in sibling order, read back after a full
/// exchange — identical on both replicas, which the function asserts.
fn sibling_peers_in_order(first: u64, second: u64, pad_on: Option<u64>) -> Vec<u64> {
    let doc_first = LoroDoc::new();
    doc_first.set_peer_id(first).unwrap();
    let doc_second = LoroDoc::new();
    doc_second.set_peer_id(second).unwrap();

    // The parent comes from one peer and is imported by the other, so both
    // replicas hold it and both are at the same Lamport afterwards.
    let tree_first = doc_first.get_tree(TREE);
    tree_first.enable_fractional_index(0);
    let parent = tree_first.create(TreeParentId::Root).unwrap();
    doc_first.commit();
    let parent_update = doc_first.export(loro::ExportMode::all_updates()).unwrap();
    doc_second.import(&parent_update).unwrap();
    let tree_second = doc_second.get_tree(TREE);
    tree_second.enable_fractional_index(0);

    for (peer, doc) in [(first, &doc_first), (second, &doc_second)] {
        if pad_on != Some(peer) {
            continue;
        }
        let pad = doc.get_map("tie_rule_pad");
        for i in 0..PAD {
            pad.insert(&format!("k{i}"), i as i64).unwrap();
            doc.commit();
        }
    }

    // Both children land under the same parent with no sibling yet, so with
    // jitter 0 both create ops carry the identical default fractional index.
    tree_first.create(TreeParentId::Node(parent)).unwrap();
    doc_first.commit();
    tree_second.create(TreeParentId::Node(parent)).unwrap();
    doc_second.commit();

    let from_first = doc_first.export(loro::ExportMode::all_updates()).unwrap();
    let from_second = doc_second.export(loro::ExportMode::all_updates()).unwrap();
    doc_first.import(&from_second).unwrap();
    doc_second.import(&from_first).unwrap();

    let order = |doc: &LoroDoc| -> Vec<u64> {
        doc.get_tree(TREE)
            .children(TreeParentId::Node(parent))
            .expect("the parent is alive on both replicas, so it has a child list")
            .into_iter()
            .map(|child: TreeID| child.peer)
            .collect()
    };
    let (order_first, order_second) = (order(&doc_first), order(&doc_second));
    assert_eq!(
        order_first, order_second,
        "the two replicas disagree on sibling order after a full exchange \
         (peers {first}/{second}, pad on {pad_on:?})"
    );
    assert_eq!(
        order_first.len(),
        2,
        "each peer contributed exactly one child (peers {first}/{second})"
    );

    // The premise of the whole test: the position really ties, so `idlp` is
    // what decides. Without this the test would pass vacuously if an upgrade
    // started handing the two creates different indices.
    let tree = doc_first.get_tree(TREE);
    let children = tree.children(TreeParentId::Node(parent)).unwrap();
    let positions: Vec<Option<String>> = children
        .iter()
        .map(|child| tree.fractional_index(*child))
        .collect();
    assert_eq!(
        positions[0], positions[1],
        "the two siblings must share one fractional index, else the position \
         decides and the tie rule is untested (peers {first}/{second})"
    );

    // And the Lamport gap is the one the case asked for.
    let lamports: Vec<u32> = children
        .iter()
        .map(|child| {
            let id = tree
                .get_last_move_id(child)
                .expect("a child that exists was created by a move op");
            let change = doc_first
                .get_change(id)
                .expect("the create op's change is in this replica's oplog");
            change.lamport + (id.counter - change.id.counter) as u32
        })
        .collect();
    let gap = lamports[1].abs_diff(lamports[0]);
    let want_gap = if pad_on.is_some() { PAD as u32 } else { 0 };
    assert_eq!(
        gap, want_gap,
        "the two create ops must differ by {want_gap} in Lamport for this case \
         (peers {first}/{second}, pad on {pad_on:?}, lamports {lamports:?})"
    );

    order_first
}

#[test]
fn equal_lamport_siblings_order_by_peer_id() {
    assert_eq!(
        sibling_peers_in_order(2, 3, None),
        vec![2, 3],
        "equal Lamport: the lower peer id comes first"
    );
    assert_eq!(
        sibling_peers_in_order(3, 2, None),
        vec![2, 3],
        "equal Lamport: the lower peer id comes first, whichever peer created \
         the parent"
    );
}

#[test]
fn a_higher_lamport_sibling_comes_last_whichever_peer_holds_it() {
    assert_eq!(
        sibling_peers_in_order(2, 3, Some(2)),
        vec![3, 2],
        "the higher Lamport sorts later even on the LOWER peer — a plain \
         'lower peer first' rule would put 2 first"
    );
    assert_eq!(
        sibling_peers_in_order(2, 3, Some(3)),
        vec![2, 3],
        "the higher Lamport sorts later on the higher peer too"
    );
}
