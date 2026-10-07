//! The UNARMED delete gate: which rows an unarmed full walk may delete.
//!
//! Before `arm()` the sink holds raw-inserted seed-layout rows Loro never
//! held, and the full walk must keep them. The row of a block Loro held and
//! tombstoned is deleted like any other: that is how the org initial scan
//! removes a headline deleted from its file while the app was off (bugfunnel
//! 2026-10-07-headline-deleted-while-off-is-written-back-at-boot).
//!
//! `LoroProjection::project`'s incremental fast path is entered on `seeded`
//! alone, and an unarmed batch that carries a delete is routed to the full
//! walk (`FullReason::UnarmedDelete`), where the gate has its one
//! implementation. The route is observable through `MemorySink::read_calls`:
//! only the full walk reads the sink (`read_sql_snapshot`) for its diff base.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-unarmed-delete — an unarmed batch carrying a
//! delete (global or layout) routes to the full walk, which deletes the row of
//! a block Loro tombstoned and keeps a row Loro never held or a not yet
//! rehydrated share holds; an unarmed batch without one stays on the fast path

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::block::Block;
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
    let (clock, engine) = holon_integration_tests::pbt::engine_views::start_views_engine();
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        Arc::new(holon_api::ConditionBus::new()),
        clock,
        engine,
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
        ["block:keep-id"],
        "Loro held the block and tombstoned it, so the unarmed walk deletes its row"
    );
    // `full_passes` is the boot's walk budget; a walk invisible to it is a
    // quadratic that no scale test can see. (nextest gives each test its own
    // process, so these process-global counters are this test's alone.)
    assert_eq!(
        holon_loro::projection_stats::snapshot().full_passes,
        2,
        "the cold-boot seed plus this one"
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
        ["block:keep-id"],
        "a layout block Loro tombstoned is deleted while unarmed too"
    );
    Ok(())
}

/// The reboot shape: the previous session's `holon.db` holds the rows of
/// `keep-id` and `gone-id`, the boot seed raw-inserted `seed-only-id`, and the
/// org initial scan tombstoned `gone-id` in Loro before the first pass.
#[tokio::test]
async fn the_unarmed_cold_boot_walk_deletes_what_loro_tombstoned_and_keeps_what_it_never_held()
-> Result<()> {
    let (_tempdir, doc_store, sink, projection) = subscribed_projection().await?;

    insert_root_block_in(&doc_store, DocScope::Global, "keep-id", "kept").await?;
    let gone =
        insert_root_block_in(&doc_store, DocScope::Global, "gone-id", "deleted on disk").await?;
    for (id, content) in [
        ("keep-id", "kept"),
        ("gone-id", "deleted on disk"),
        ("seed-only-id", "a seed layout row"),
    ] {
        sink.plant_row(Block::new_text(
            EntityUri::block(id),
            EntityUri::no_parent(),
            content,
        ));
    }
    delete_block_in(&doc_store, DocScope::Global, gone).await?;

    assert!(!projection.is_armed());
    assert_eq!(projection.project().await?, ProjectionPass::Converged);
    assert_eq!(
        sink.row_ids(),
        ["block:keep-id", "block:seed-only-id"],
        "the unarmed walk deletes the row of the block Loro tombstoned and keeps the row Loro \
         never held"
    );
    Ok(())
}

/// The reboot shape of a persisted block share: sharing tombstoned the subtree
/// in the global doc and left a mount in its place, and the boot walk runs
/// before rehydration registers the shared doc again.
#[tokio::test]
async fn the_unarmed_cold_boot_walk_keeps_the_rows_of_an_unregistered_share() -> Result<()> {
    let (_tempdir, doc_store, sink, projection) = subscribed_projection().await?;
    let projection = projection.with_shared_trees(Arc::new(
        holon_loro::shared_tree::InMemorySharedTreeStore::new(),
    ));

    let host = insert_root_block_in(&doc_store, DocScope::Global, "host-id", "host").await?;
    let collab = doc_store.read().await.get_doc(DocScope::Global).await?;
    collab.with_write(holon_loro::WriteOrigin::Probe("share"), |txn| {
        let tree = txn.get_tree(holon_loro::TREE_NAME);
        let mut parent = host;
        let mut shared_root = None;
        for id in ["shared-root-id", "shared-kid-id"] {
            let node = tree.create(Some(parent))?;
            holon_loro::write_stable_id(txn, node, id)?;
            let meta = tree.get_meta(node)?;
            meta.insert(holon_loro::CONTENT_TYPE, loro::LoroValue::from("text"))?;
            meta.ensure_mergeable_text(holon_loro::CONTENT_RAW)?
                .insert(0, id)?;
            shared_root.get_or_insert(node);
            parent = node;
        }
        let extracted = holon_loro::shared_tree::extract_for_share(
            txn,
            shared_root.expect("the loop created the shared root"),
            Some(host),
            "share-id".into(),
            holon_loro::shared_tree::HistoryRetention::None,
        )?;
        let mount = holon_loro::shared_tree::commit_share_prune(txn, &extracted)?;
        holon_loro::write_stable_id(txn, mount, "container-id")?;
        holon_loro::shared_tree::record_mount(
            &tree,
            mount,
            &holon_loro::shared_tree::ShareKind::Block,
            holon_loro::shared_tree::MountRole::Owner,
        )?;
        Ok(())
    })?;
    for (id, parent) in [
        ("host-id", EntityUri::no_parent()),
        ("container-id", EntityUri::block("host-id")),
        ("shared-root-id", EntityUri::block("container-id")),
        ("shared-kid-id", EntityUri::block("shared-root-id")),
    ] {
        sink.plant_row(Block::new_text(EntityUri::block(id), parent, id));
    }

    assert!(!projection.is_armed());
    assert_eq!(projection.project().await?, ProjectionPass::Converged);
    assert_eq!(
        sink.row_ids(),
        [
            "block:container-id",
            "block:host-id",
            "block:shared-kid-id",
            "block:shared-root-id"
        ],
        "the shared blocks are tombstoned in the global doc but live in the share the mount \
         places, so the walk before rehydration keeps their rows"
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
