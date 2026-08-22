//! The comparator, checked against the order LogSeq itself stored.
//!
//! A tree's leaves ARE its sorted order, so walking the committed fixture's
//! three indexes yields LogSeq's own sequence with no oracle involved. If
//! Holon's comparator agrees that every one of those sequences is strictly
//! increasing, it orders datoms the way LogSeq does — which is the whole
//! precondition for inserting one in the right place.
//!
//! The rules under test are written up, with their measurements, in
//! docs/Testing/LogseqDbTreeOrder.md.

use std::path::Path;
use std::path::PathBuf;

use holon_logseq_db::kvs_writer;
use holon_logseq_db::tree::Index;
use holon_logseq_db::tree::Tree;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

/// No index of the real graph is DECIDABLY out of order under our comparator,
/// and the only pairs it cannot decide are the documented ones.
///
/// Two separate claims, because they are separately falsifiable:
///
/// 1. Wherever the comparator has an opinion, that opinion is `Less` —
///    strictly, since two datoms comparing equal would be the same datom, and a
///    tie means an insert could land anywhere between them.
/// 2. The pairs it refuses are exactly map-against-map, which is the case
///    ClojureScript settles by `hash` and this build deliberately does not
///    implement. eavt and aevt must have NONE: their value slot is reached only
///    when `(e,a)` / `(a,e)` already tie.
///
/// The refusals are expected, not a shortfall. VERIFYING an existing tree's
/// full order needs the hash; INSERTING a non-map datom into it does not,
/// because that comparison is settled by the type group. That asymmetry is
/// the whole basis of the ruling in docs/Testing/LogseqDbTreeOrder.md.
#[tokio::test]
async fn no_index_of_the_fixture_is_decidably_out_of_order() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    for index in [Index::Eavt, Index::Aevt, Index::Avet] {
        let tree = Tree::load(&graph, index).expect("tree loads");
        let datoms = tree.datoms().expect("leaves walk in order");

        assert!(
            !datoms.is_empty(),
            "{index:?} walked to nothing, so the assertions below would be vacuous"
        );

        let mut undecidable = 0usize;
        for pair in datoms.windows(2) {
            match index.compare(&pair[0], &pair[1]) {
                Ok(std::cmp::Ordering::Less) => {}
                Ok(other) => panic!(
                    "{index:?} is not strictly increasing ({other:?}) at\n  {:?}\n  {:?}",
                    pair[0], pair[1]
                ),
                Err(kvs_writer::RowError::ValueNotOrderable { kind: "map" }) => undecidable += 1,
                Err(e) => panic!("{index:?}: unexpected refusal: {e}\n  {:?}", pair[0]),
            }
        }

        // Asserted, not merely printed: the number of pairs we cannot decide
        // is a property of this graph, so a change in it is a change in what
        // the writer is allowed to assume — a red, not drift to be noticed
        // later. eavt/aevt reach a value comparison only after (e,a) / (a,e)
        // already tie, so theirs must be zero.
        // 4, not 5: one of avet's map-valued neighbours holds the SAME map as
        // its predecessor, and equal values compare equal without needing an
        // order — datascript's `value-compare` tests equality first for the
        // same reason. Only genuinely DIFFERENT maps are undecidable.
        let expected = match index {
            Index::Eavt | Index::Aevt => 0,
            Index::Avet => 4,
        };
        assert_eq!(
            undecidable, expected,
            "{index:?}: expected {expected} adjacent pair(s) undecidable without the cljs \
             hash, found {undecidable}"
        );
        println!("{index:?}: {undecidable} adjacent pair(s) undecidable without the cljs hash");
    }
}

/// The walk finds exactly the datoms addr 0 says each index holds.
///
/// Guards the traversal rather than the comparator: a walk that silently
/// skipped a subtree would still be "sorted", and every later claim about
/// where a datom belongs would be made against a partial tree.
#[tokio::test]
async fn each_index_walks_the_count_addr_0_declares() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    for (index, expected) in [
        (Index::Eavt, graph.root.eavt_metadata.count),
        (Index::Aevt, graph.root.aevt_metadata.count),
        (Index::Avet, graph.root.avet_metadata.count),
    ] {
        let tree = Tree::load(&graph, index).expect("tree loads");
        let datoms = tree.datoms().expect("leaves walk");
        assert_eq!(
            datoms.len() as i64,
            expected,
            "{index:?} walked {} datoms but addr 0 declares {expected}",
            datoms.len()
        );
    }
}

/// The measured depth: shift is depth − 1, and the fixture's shift is 2.
#[tokio::test]
async fn the_walk_agrees_with_the_declared_depth() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");
    let tree = Tree::load(&graph, Index::Eavt).expect("tree loads");
    assert_eq!(
        tree.depth().expect("depth") as i64,
        graph.root.eavt_metadata.shift + 1,
        "shift is depth - 1"
    );
}
