//! The armed projection's withheld DELETE is owed to the sink.
//!
//! `LoroProjection::project` withholds deletes whenever the snapshot is
//! unsettled — a live node whose `STABLE_ID` has not landed under-reports the
//! live set, so a block that is still there would look deleted. The row that
//! must go therefore stays in SQL, and only a later pass can remove it;
//! `ProjectionPass` is the only place the caller can read that.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-withheld-delete — an armed unsettled pass owes
//! the delete it withheld

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_core::DownstreamProjection;
use holon_core::OriginTaggedWrites;
use holon_core::ProjectionPass;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use loro::Frontiers;
use loro::TreeID;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;
use crate::projection_harness::insert_root_block;

/// Create a live tree node and commit it WITHOUT `STABLE_ID` meta — the state a
/// concurrent reader observes between a create's node step and its meta step,
/// which makes the snapshot unsettled.
async fn insert_half_born_node(doc_store: &Arc<RwLock<LoroDocumentStore>>) -> Result<TreeID> {
    let collab = doc_store.read().await.get_doc(DocScope::Global).await?;
    collab.with_write(|txn| Ok(txn.get_tree(TREE_NAME).create(None)?))
}

async fn delete_node(doc_store: &Arc<RwLock<LoroDocumentStore>>, node: TreeID) -> Result<()> {
    let collab = doc_store.read().await.get_doc(DocScope::Global).await?;
    collab.with_write(|txn| Ok(txn.get_tree(TREE_NAME).delete(node)?))
}

#[tokio::test]
async fn an_armed_unsettled_pass_owes_the_delete_it_withheld() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    doc_store.read().await.get_doc(DocScope::Global).await?;

    let sink = Arc::new(MemorySink::new());
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
    );
    projection.arm();

    insert_root_block(&doc_store, "keep-id", "kept").await?;
    let gone = insert_root_block(&doc_store, "gone-id", "removed later").await?;
    assert_eq!(
        projection.project().await?,
        ProjectionPass::Converged,
        "the seed pass writes both rows whole"
    );
    assert_eq!(sink.row_ids(), ["block:gone-id", "block:keep-id"]);

    // The row's block leaves the tree, and a half-born sibling makes the
    // snapshot that would carry the delete unsettled.
    delete_node(&doc_store, gone).await?;
    let half_born = insert_half_born_node(&doc_store).await?;

    let pass = projection.flush().await.expect("the pass itself succeeds");
    assert_eq!(
        pass,
        ProjectionPass::Incomplete { withheld: 1 },
        "the withheld delete is owed to the sink, so the pass did not converge"
    );
    assert!(
        sink.row_ids().contains(&"block:gone-id".to_string()),
        "the delete really was withheld — the row is still there: {:?}",
        sink.row_ids()
    );
    assert!(
        !projection.is_seeded(),
        "a pass that owes an op stays on the full walk against sink truth"
    );

    // Once the snapshot settles, the same delete is emittable: the withhold
    // deferred it rather than dropping it.
    delete_node(&doc_store, half_born).await?;
    assert_eq!(
        projection.flush().await.expect("the settled pass succeeds"),
        ProjectionPass::Converged
    );
    assert_eq!(sink.row_ids(), ["block:keep-id"]);

    Ok(())
}
