//! Wall time of an ingest-shaped create (a lookup that misses, then one write
//! batch that creates the node) must not grow with the tree, in any build
//! profile. Prints the per-create time of each bucket.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_loro::LoroDocument;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::write_stable_id;

const CREATES: usize = 6000;
const BUCKET: usize = 1000;

#[test]
fn a_create_costs_the_same_in_a_large_tree_as_in_a_small_one() {
    let doc = Arc::new(LoroDocument::new("stable-id-create-cost".into()).unwrap());
    let backend = LoroBackend::from_document(doc.clone());
    let root = doc
        .with_write(WriteOrigin::BlockOps, |d| {
            let root = d.get_tree(TREE_NAME).create(None)?;
            write_stable_id(d, root, "root")?;
            Ok(root)
        })
        .unwrap();

    let mut buckets = Vec::new();
    let mut bucket_start = Instant::now();
    for k in 0..CREATES {
        let sid = format!("n{k}");
        assert!(backend.find_tree_id_by_stable_id_sync(&sid).is_none());
        let node = doc
            .with_write(WriteOrigin::BlockOps, |d| {
                let node = d.get_tree(TREE_NAME).create(root)?;
                write_stable_id(d, node, &sid)?;
                Ok(node)
            })
            .unwrap();
        assert_eq!(backend.find_tree_id_by_stable_id_sync(&sid), Some(node));
        if (k + 1) % BUCKET == 0 {
            buckets.push(bucket_start.elapsed() / BUCKET as u32);
            bucket_start = Instant::now();
        }
    }

    let profile = if cfg!(debug_assertions) {
        "debug_assertions on"
    } else {
        "debug_assertions off"
    };
    for (b, per_create) in buckets.iter().enumerate() {
        eprintln!(
            "[{profile}] creates {}..{}: {per_create:?} per create",
            b * BUCKET,
            (b + 1) * BUCKET
        );
    }
    let early = buckets[0];
    let late = *buckets.last().unwrap();
    assert!(
        late <= early * 3 + Duration::from_micros(50),
        "[{profile}] a create in a tree of {CREATES} nodes takes {late:?}, in a tree of \
         {BUCKET} nodes {early:?}: the per-create cost grows with the tree"
    );
}
