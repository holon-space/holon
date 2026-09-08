//! The UNARMED delete keeps its gate after the fast path stopped checking
//! `armed`.
//!
//! `LoroProjection::project`'s incremental fast path is entered on `seeded`
//! alone — `armed` gates DELETES, not creates/updates, and gating the fast path
//! on it made every org-scan commit walk the whole accumulated tree. The delete
//! gate therefore has exactly one implementation, on the full walk, and an
//! unarmed batch that carries a delete is routed there
//! (`FullReason::UnarmedDelete`). These tests pin that routing: without it an
//! unarmed delete would reach the sink through the fast path, wiping the
//! SQL-only seed-layout rows the gate exists to protect.
//!
//! The route is observable through `MemorySink::read_calls`: only the full walk
//! reads the sink (`read_sql_snapshot`) for its diff base.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-unarmed-delete — an unarmed batch carrying a
//! delete (global or layout) routes to the full walk, which withholds it; an
//! unarmed batch without one stays on the fast path

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
use loro::Frontiers;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;
use crate::projection_harness::delete_block_in;
use crate::projection_harness::insert_root_block_in;

/// A projection over a fresh store with its doc subscriptions installed — the
/// production wiring, where the fast path can actually be reached because the
/// pending-facts queue is being fed.
async fn subscribed_projection() -> Result<(
    TempDir,
    Arc<RwLock<LoroDocumentStore>>,
    Arc<MemorySink>,
    LoroProjection,
)> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    let sink = Arc::new(MemorySink::new());
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
    );
    projection.install_doc_subscriptions().await?;
    Ok((tempdir, doc_store, sink, projection))
}

#[tokio::test]
async fn an_unarmed_global_delete_routes_to_the_full_walk_which_withholds_it() -> Result<()> {
    let (_tempdir, doc_store, sink, projection) = subscribed_projection().await?;

    insert_root_block_in(&doc_store, DocScope::Global, "keep-id", "kept").await?;
    let gone =
        insert_root_block_in(&doc_store, DocScope::Global, "gone-id", "removed later").await?;
    assert_eq!(projection.project().await?, ProjectionPass::Converged);
    assert!(
        projection.is_seeded(),
        "the cold-boot walk seeded the incremental base, so the next pass is eligible for the \
         fast path"
    );
    assert_eq!(sink.row_ids(), ["block:gone-id", "block:keep-id"]);

    let reads = sink.read_calls();
    delete_block_in(&doc_store, DocScope::Global, gone).await?;
    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.read_calls(),
        reads + 1,
        "the pass read the sink, so it took the full walk — the fast path never does"
    );
    assert_eq!(
        sink.row_ids(),
        ["block:gone-id", "block:keep-id"],
        "the unarmed delete gate held: the row is still there"
    );
    // This walk withheld its only op, so it emitted NONE — and it must still be
    // counted. `full_passes` is the boot's walk budget; a walk invisible to it
    // is a quadratic that no scale test can see. (nextest gives each test its
    // own process, so these process-global counters are this test's alone.)
    assert_eq!(
        holon_loro::projection_stats::snapshot().full_passes,
        2,
        "the zero-op full walk counts: the cold-boot seed plus this one"
    );
    Ok(())
}

#[tokio::test]
async fn an_unarmed_layout_delete_routes_to_the_full_walk_too() -> Result<()> {
    let (_tempdir, doc_store, sink, projection) = subscribed_projection().await?;

    insert_root_block_in(&doc_store, DocScope::Global, "keep-id", "kept").await?;
    let gone =
        insert_root_block_in(&doc_store, DocScope::Layout, "layout-gone-id", "a pane").await?;
    assert_eq!(projection.project().await?, ProjectionPass::Converged);
    assert_eq!(sink.row_ids(), ["block:keep-id", "block:layout-gone-id"]);

    let reads = sink.read_calls();
    delete_block_in(&doc_store, DocScope::Layout, gone).await?;
    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    // The layout doc's changes are merged into `changed` BEFORE the batch's
    // deletes are counted. Counting them earlier would let a layout-only delete
    // slip through the fast path with `deletes == 0`.
    assert_eq!(
        sink.read_calls(),
        reads + 1,
        "a layout-only delete is still an unarmed delete: full walk"
    );
    assert_eq!(
        sink.row_ids(),
        ["block:keep-id", "block:layout-gone-id"],
        "the unarmed delete gate held for the layout row too"
    );
    Ok(())
}

#[tokio::test]
async fn an_unarmed_batch_without_a_delete_stays_on_the_fast_path() -> Result<()> {
    let (_tempdir, doc_store, sink, projection) = subscribed_projection().await?;

    insert_root_block_in(&doc_store, DocScope::Global, "keep-id", "kept").await?;
    assert_eq!(projection.project().await?, ProjectionPass::Converged);
    assert!(projection.is_seeded());

    let reads = sink.read_calls();
    insert_root_block_in(&doc_store, DocScope::Global, "new-id", "arrived later").await?;
    // Through `flush` — the entry point the org initial scan uses, once per
    // file, which is where the quadratic was paid.
    assert_eq!(
        projection.flush().await.expect("the flush succeeds"),
        ProjectionPass::Converged
    );

    assert_eq!(
        sink.read_calls(),
        reads,
        "an unarmed CREATE must not pay a full-document walk — that is the org scan's quadratic"
    );
    assert_eq!(sink.row_ids(), ["block:keep-id", "block:new-id"]);
    let stats = holon_loro::projection_stats::snapshot();
    assert_eq!(
        (stats.passes, stats.full_passes),
        (2, 1),
        "two passes, only the cold-boot seed a full walk"
    );
    Ok(())
}
