//! Editing an index tree, checked against a trivially-correct reference.
//!
//! The reference is a sorted `Vec`: inserting into it is "find the position
//! and splice", which has no split, no merge and nothing to get wrong. If the
//! B+-tree agrees with that Vec after a few hundred edits AND still satisfies
//! its own structural invariants, then split, rotate, merge and merge-n-split
//! are doing what they claim.
//!
//! The edits are deliberately chosen to force BOTH paths: enough inserts into
//! one region to overflow leaves, and enough removals to underflow them.

use std::path::Path;
use std::path::PathBuf;

use holon_logseq_db::TransitNode;
use holon_logseq_db::kvs_writer;
use holon_logseq_db::tree::EditableTree;
use holon_logseq_db::tree::Index;
use holon_logseq_db::tree::TreeDatom;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

/// Insert into a sorted vector at the position the index's own comparator
/// gives — the reference implementation.
fn reference_insert(index: Index, seq: &mut Vec<TreeDatom>, datom: &TreeDatom) {
    let mut at = seq.len();
    for (i, existing) in seq.iter().enumerate() {
        if index.compare(datom, existing).expect("comparable") == std::cmp::Ordering::Less {
            at = i;
            break;
        }
    }
    seq.insert(at, datom.clone());
}

/// Synthetic datoms on a string-valued attribute, so every comparison is
/// decidable and nothing needs ClojureScript's hash.
fn synthetic(i: usize) -> TreeDatom {
    TreeDatom {
        e: 900_000 + i as i64,
        a: "block/title".to_string(),
        v: TransitNode::Str(format!("tree edit probe {i:04}")),
        tx: 536_900_000 + i as i64,
    }
}

async fn load(index: Index) -> EditableTree {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");
    EditableTree::load(&graph, index).expect("tree loads")
}

/// Inserting many datoms keeps the tree equal to the reference sequence, and
/// keeps it a valid B+-tree. 400 inserts on one attribute forces leaf splits
/// and at least one branch split.
#[tokio::test]
async fn inserting_agrees_with_a_sorted_vector() {
    for index in [Index::Eavt, Index::Aevt, Index::Avet] {
        let mut tree = load(index).await;
        let mut reference = tree.datoms();

        for i in 0..400 {
            let datom = synthetic(i);
            assert!(
                tree.insert(&datom).expect("insert"),
                "insert {i} was a no-op"
            );
            reference_insert(index, &mut reference, &datom);
            if i % 97 == 0 {
                tree.check_invariants()
                    .unwrap_or_else(|e| panic!("{index:?} after insert {i}: {e}"));
            }
        }

        tree.check_invariants()
            .unwrap_or_else(|e| panic!("{index:?}: {e}"));
        assert_eq!(
            tree.datoms(),
            reference,
            "{index:?} disagrees with the reference sequence after 400 inserts"
        );
    }
}

/// Inserting the same datom twice is a no-op, in the tree and in the count.
#[tokio::test]
async fn inserting_a_datom_that_is_already_there_changes_nothing() {
    let mut tree = load(Index::Eavt).await;
    let before = tree.datoms();
    let datom = before[before.len() / 2].clone();

    assert!(
        !tree.insert(&datom).expect("insert"),
        "an existing datom must report that it was already present"
    );
    assert_eq!(tree.datoms(), before, "and must not change the sequence");
}

/// Removing every datom we added gets the tree back to where it started —
/// exercising underflow, merge and merge-n-split on the way down.
#[tokio::test]
async fn removing_undoes_inserting() {
    for index in [Index::Eavt, Index::Aevt, Index::Avet] {
        let mut tree = load(index).await;
        let original = tree.datoms();

        for i in 0..400 {
            tree.insert(&synthetic(i)).expect("insert");
        }
        tree.check_invariants()
            .unwrap_or_else(|e| panic!("{index:?} after inserts: {e}"));

        // Removed in a different order than inserted, so the merges do not
        // simply undo the splits in reverse.
        for i in (0..400).rev().step_by(3).chain((0..400).step_by(1)) {
            let datom = synthetic(i);
            tree.remove(&datom).expect("remove");
        }
        tree.check_invariants()
            .unwrap_or_else(|e| panic!("{index:?} after removes: {e}"));

        assert_eq!(
            tree.datoms(),
            original,
            "{index:?} did not return to its original sequence"
        );
    }
}

/// Removing something that is not there reports so and leaves the tree alone.
#[tokio::test]
async fn removing_an_absent_datom_is_a_no_op() {
    let mut tree = load(Index::Eavt).await;
    let before = tree.datoms();

    assert!(
        !tree.remove(&synthetic(0)).expect("remove"),
        "removing an absent datom must report that it was absent"
    );
    assert_eq!(tree.datoms(), before);
}

/// The fixture's own trees satisfy the invariants before anything is edited.
///
/// Without this the edit tests could be measuring a tree that was already
/// malformed when it was loaded.
#[tokio::test]
async fn the_fixture_trees_are_valid_before_any_edit() {
    for index in [Index::Eavt, Index::Aevt, Index::Avet] {
        let tree = load(index).await;
        tree.check_invariants()
            .unwrap_or_else(|e| panic!("{index:?} is malformed as loaded: {e}"));
    }
}

/// Deleting down to a handful of datoms LOSES levels, as LogSeq's own storage
/// does.
///
/// MEASURED first, then encoded here: a storage-backed graph grown to 1500
/// datoms reports `shift` 2, and retracting to 500 then to 3 reports 1 then 0
/// — and shift is depth − 1 (scratchpad/logs/root-collapse.log). A writer that
/// never collapsed would keep building deeper trees than LogSeq's for exactly
/// the same datoms.
#[tokio::test]
async fn removing_almost_everything_collapses_the_root() {
    let mut tree = load(Index::Eavt).await;
    let original = tree.datoms();
    let deep = tree.depth().expect("depth");
    assert!(
        deep >= 3,
        "the fixture should start several levels deep, got {deep}"
    );

    // Keep only a handful, deleting in stored order.
    for datom in original.iter().take(original.len() - 5) {
        tree.remove(datom).expect("remove");
    }

    tree.check_invariants()
        .unwrap_or_else(|e| panic!("after collapsing: {e}"));
    let shallow = tree.depth().expect("depth");
    assert!(
        shallow < deep,
        "the tree kept {shallow} level(s) for 5 datoms, having started with {deep}; \
         a single-child root must collapse"
    );
    assert_eq!(
        tree.datoms().len(),
        5,
        "and the survivors are exactly the datoms that were not removed"
    );
}

// ------------------------------------------------- the writer path is safe

/// Serializing an UNEDITED tree reproduces the rows it was read from, byte
/// for byte, and allocates no addresses.
///
/// The identity check the whole writer rests on: if loading and re-emitting a
/// tree cannot reproduce it exactly, then every difference after a real edit
/// is ambiguous — was it the edit, or was it the writer? Run before any
/// mutation path is trusted.
#[tokio::test]
async fn serializing_an_unedited_tree_reproduces_its_rows_exactly() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    let mut covered = 0usize;
    for index in [Index::Eavt, Index::Aevt, Index::Avet] {
        let tree = EditableTree::load(&graph, index).expect("tree loads");
        let out = tree.serialize(graph.root.max_addr).expect("serializes");

        assert_eq!(
            out.max_addr, graph.root.max_addr,
            "{index:?} allocated an address for a tree nobody edited"
        );
        assert_eq!(
            out.root_addr,
            index.root_addr(&graph),
            "{index:?} moved its root without being asked to"
        );

        for node in &out.nodes {
            let original = graph
                .rows
                .iter()
                .find(|r| r.addr == node.addr)
                .unwrap_or_else(|| panic!("{index:?}: emitted addr {} is not a row", node.addr));

            assert_eq!(
                holon_logseq_db::encode_document(&node.node),
                original.original_content,
                "{index:?} addr {}: re-emitted content differs from the bytes on disk",
                node.addr
            );
            assert_eq!(
                node.addresses, original.addresses,
                "{index:?} addr {}: re-emitted child pointers differ",
                node.addr
            );
            covered += 1;
        }
    }

    // The three trees do NOT account for every row: the fixture carries
    // unreferenced ones, left behind when LogSeq merged nodes away and
    // discarded its own delete list. Measured at 17 on this graph.
    //
    // Pinned rather than waved at, because it is a property a writer must not
    // get wrong in either direction: emitting FEWER than the reachable nodes
    // would mean a subtree went unserialized (and the byte comparisons above
    // would have been over a partial tree), while treating every row as
    // reachable would resurrect garbage.
    let tree_rows = graph.rows.len() - 2; // minus the head (0) and the tail (1)
    let orphans = tree_rows - covered;
    assert_eq!(
        orphans, 17,
        "expected 17 unreferenced rows in the fixture, found {orphans} \
         ({covered} of {tree_rows} tree rows reachable from the three roots)"
    );
}
