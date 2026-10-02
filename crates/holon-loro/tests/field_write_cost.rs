//! Cost oracle of a scalar field write through `BlockCellRegistry`: the tree
//! nodes it reads, and its time, do not grow with the number N of blocks in
//! the doc.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_api::Value;
use holon_core::cell_registry::EntityCellRegistry;
use holon_loro::LoroDocument;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::block_cell_registry::BlockCellRegistry;
use holon_loro::write_stable_id;

const SMALL: usize = 500;
const LARGE: usize = 20_000;
const WRITES: usize = 40;

struct Measured {
    full_builds: u64,
    nodes_visited_per_write: f64,
    fastest_write: Duration,
}

/// `n` blocks under 20 roots, the shape of a vault's pages and headlines.
fn seeded(n: usize) -> Arc<LoroDocument> {
    let doc = Arc::new(LoroDocument::new("global".into()).unwrap());
    doc.with_write(WriteOrigin::BlockOps, |d| {
        let tree = d.get_tree(TREE_NAME);
        let roots = (0..20)
            .map(|p| {
                let root = tree.create(None)?;
                write_stable_id(d, root, &format!("page-{p}"))?;
                Ok(root)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        for k in 0..n {
            let node = tree.create(roots[k % roots.len()])?;
            write_stable_id(d, node, &format!("b{k}"))?;
        }
        Ok(())
    })
    .unwrap();
    doc
}

async fn measure(n: usize) -> Measured {
    let doc = seeded(n);
    let layout = Arc::new(LoroDocument::new("layout".into()).unwrap());
    let registry = BlockCellRegistry::with_loro(
        doc.clone(),
        layout,
        Arc::new(holon_core::NoReadOnlyDocuments),
    );
    let write = |k: usize| {
        let uri = EntityUri::block(&format!("b{}", (k * 7919) % n));
        let registry = &registry;
        async move {
            let routed = registry
                .write_field(&uri, "completed", Value::Boolean(k.is_multiple_of(2)))
                .await
                .unwrap();
            assert!(
                routed,
                "the write of {uri} must route through the Loro cell"
            );
        }
    };
    write(0).await;
    let before = doc.stable_id_index_stats().unwrap();
    let mut fastest = Duration::MAX;
    for k in 1..=WRITES {
        let start = Instant::now();
        write(k).await;
        fastest = fastest.min(start.elapsed());
    }
    let after = doc.stable_id_index_stats().unwrap();
    Measured {
        full_builds: after.full_builds - before.full_builds,
        nodes_visited_per_write: (after.nodes_visited - before.nodes_visited) as f64
            / WRITES as f64,
        fastest_write: fastest,
    }
}

#[tokio::test]
async fn a_field_write_reads_a_number_of_nodes_constant_in_the_doc_size() {
    let small = measure(SMALL).await;
    let large = measure(LARGE).await;
    eprintln!(
        "N={SMALL}: {:.1} nodes/write, fastest {:?}; N={LARGE}: {:.1} nodes/write, fastest {:?}",
        small.nodes_visited_per_write,
        small.fastest_write,
        large.nodes_visited_per_write,
        large.fastest_write
    );
    assert_eq!(
        (small.full_builds, large.full_builds),
        (0, 0),
        "a field write after the first lookup rebuilt the stable-id index"
    );
    assert!(
        large.nodes_visited_per_write <= 2.0,
        "a field write read {:.1} index nodes at N={LARGE}",
        large.nodes_visited_per_write
    );
    // The minimum over many writes is the algorithmic cost; host load only adds to
    // it.
    assert!(
        large.fastest_write < small.fastest_write * 5,
        "a field write grows with N: fastest {:?} at N={SMALL}, {:?} at N={LARGE}",
        small.fastest_write,
        large.fastest_write
    );
}
