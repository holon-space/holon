//! Cost oracle of the stable-id index: after the first build, a lookup reads
//! only the subtrees of the nodes changed since the last lookup, never the
//! whole tree.

use std::sync::Arc;

use holon_loro::LoroDocument;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::write_stable_id;
use loro::TreeID;
use proptest::prelude::*;

const PARENTS: usize = 10;

#[derive(Debug, Clone, Copy)]
enum Change {
    Create,
    Delete,
    Move,
    Rewrite,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        Just(Change::Create),
        Just(Change::Delete),
        Just(Change::Move),
        Just(Change::Rewrite),
    ]
}

/// `PARENTS` roots, each with leaves; returns the roots and the leaves.
fn seed(doc: &LoroDocument, leaves: usize) -> (Vec<TreeID>, Vec<TreeID>) {
    doc.with_write(WriteOrigin::BlockOps, |d| {
        let tree = d.get_tree(TREE_NAME);
        let mut roots = Vec::new();
        for p in 0..PARENTS {
            let root = tree.create(None)?;
            write_stable_id(d, root, &format!("p{p}"))?;
            roots.push(root);
        }
        let mut nodes = Vec::new();
        for k in 0..leaves {
            let leaf = tree.create(roots[k % PARENTS])?;
            write_stable_id(d, leaf, &format!("n{k}"))?;
            nodes.push(leaf);
        }
        Ok((roots, nodes))
    })
    .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8, failure_persistence: None, .. ProptestConfig::default() })]

    #[test]
    fn a_lookup_after_k_changes_reads_only_the_changed_subtrees(
        leaves in 200usize..2000,
        changes in proptest::collection::vec(change(), 1..20),
    ) {
        let doc = Arc::new(LoroDocument::new("stable-id-cost".into()).unwrap());
        let backend = LoroBackend::from_document(doc.clone());
        let (roots, mut nodes) = seed(&doc, leaves);

        prop_assert!(backend.find_tree_id_by_stable_id_sync("n0").is_some());
        let built = doc.stable_id_index_stats().unwrap();
        prop_assert_eq!(built.full_builds, 1, "the first lookup builds the index once");

        for (step, change) in changes.iter().enumerate() {
            let leaf = nodes[step % nodes.len()];
            doc.with_write(WriteOrigin::BlockOps, |d| {
                let tree = d.get_tree(TREE_NAME);
                match change {
                    Change::Create => {
                        let node = tree.create(roots[step % PARENTS])?;
                        write_stable_id(d, node, &format!("c{step}"))?;
                        nodes.push(node);
                    }
                    Change::Delete => {
                        tree.delete(leaf)?;
                        nodes.retain(|n| *n != leaf);
                    }
                    Change::Move => tree.mov(leaf, roots[(step + 1) % PARENTS])?,
                    Change::Rewrite => write_stable_id(d, leaf, &format!("r{step}"))?,
                }
                Ok(())
            })
            .unwrap();
        }
        backend.find_tree_id_by_stable_id_sync("n1");

        let after = doc.stable_id_index_stats().unwrap();
        prop_assert_eq!(after.full_builds, 1, "no whole-tree build after the first");
        let visited = after.nodes_visited - built.nodes_visited;
        // Every change touches one leaf (a subtree of one node); a write
        // through the chokepoint notes its node a second time.
        prop_assert!(
            visited <= 2 * changes.len() as u64,
            "{visited} nodes read for {} leaf changes in a tree of {} nodes",
            changes.len(),
            leaves + PARENTS
        );
    }
}
