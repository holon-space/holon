//! Every commit stamp the projected docs' subscriptions mint is fed by the
//! time the projection settles. An outstanding stamp holds the commit clock's
//! low watermark, so no later commit from any store could be reported applied.
//!
//! @pbt kind harness
//! @pbt covers loro-commit-clock-fed — every minted Loro stamp is fed at
//! settle, over the full walk, the incremental path and the idle return, each
//! by its own pass

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_api::commit_clock::CommitSource;
use holon_core::OriginTaggedWrites;
use holon_core::ProjectionPass;
use holon_integration_tests::pbt::engine_views::engine_caught_up;
use holon_loro::CONTENT_RAW;
use holon_loro::DocScope;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_views::engine::ViewEngine;
use loro::Frontiers;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;

const MAX_PASSES: usize = 50;

fn insert(doc: &LoroDocument, id: &str) -> Result<()> {
    doc.with_write(WriteOrigin::Probe("stamps_fed"), |txn| {
        let tree = txn.get_tree(TREE_NAME);
        let node = tree.create(None)?;
        holon_loro::write_stable_id(txn, node, id)?;
        tree.get_meta(node)?
            .ensure_mergeable_text(CONTENT_RAW)?
            .insert(0, "x")?;
        Ok(())
    })
}

/// A commit outside the block tree: the oplog moves, no block fact is queued.
fn touch_outside_the_tree(doc: &LoroDocument, n: usize) -> Result<()> {
    doc.with_write(WriteOrigin::Probe("stamps_fed"), |txn| {
        txn.get_map("stamps_fed_probe").insert("n", n as i64)?;
        Ok(())
    })
}

fn assert_fed(projection: &LoroProjection, stage: &str) {
    let clock = projection.commit_clock();
    let unfed: Vec<_> = [CommitSource::LoroGlobal, CommitSource::LoroLayout]
        .into_iter()
        .map(|s| (s, clock.outstanding(s)))
        .filter(|(_, o)| !o.is_empty())
        .collect();
    assert!(
        unfed.is_empty(),
        "{stage}: stamps never fed {unfed:?} (high water {:?}, low watermark {:?})",
        clock.high_water(),
        clock.low_watermark()
    );
    assert!(
        clock.high_water().get() > 0,
        "{stage}: the subscriptions minted nothing"
    );
}

/// Exactly one pass, so each feed point is the only one that can feed the
/// stamps minted before it.
async fn one_pass_feeds(
    projection: &LoroProjection,
    engine: &ViewEngine,
    global: &LoroDocument,
    layout: &LoroDocument,
    stage: &str,
) -> Result<()> {
    assert_eq!(
        projection.project().await?,
        ProjectionPass::Converged,
        "{stage}"
    );
    engine_caught_up(engine)?;
    assert_fed(projection, stage);
    let frontiers = |doc: &LoroDocument| doc.with_read(|d| Ok(d.oplog_frontiers()));
    assert!(
        projection.is_settled_at(&frontiers(global)?, &frontiers(layout)?),
        "{stage}: not settled after its pass"
    );
    Ok(())
}

/// Deliveries that neither queue a block fact nor move the oplog: the pass
/// takes its idle return.
fn check_out_and_back(doc: &LoroDocument, earlier: &Frontiers) -> Result<()> {
    doc.with_write(WriteOrigin::Probe("stamps_fed"), |txn| {
        txn.checkout(earlier)?;
        txn.checkout_to_latest();
        Ok(())
    })
}

async fn settle_and_assert_fed(
    projection: &LoroProjection,
    engine: &ViewEngine,
    global: &LoroDocument,
    layout: &LoroDocument,
    stage: &str,
) -> Result<()> {
    for _ in 0..MAX_PASSES {
        projection.project().await?;
        engine_caught_up(engine)?;
        let frontiers = |doc: &LoroDocument| doc.with_read(|d| Ok(d.oplog_frontiers()));
        if projection.is_settled_at(&frontiers(global)?, &frontiers(layout)?) {
            assert_fed(projection, stage);
            return Ok(());
        }
    }
    panic!("{stage}: not settled after {MAX_PASSES} passes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_minted_stamp_is_fed_at_settle() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    let global = doc_store.read().await.get_doc(DocScope::Global).await?;
    let layout = doc_store.read().await.get_doc(DocScope::Layout).await?;
    let sink = Arc::new(MemorySink::new());
    let (clock, engine) = holon_integration_tests::pbt::engine_views::start_views_engine();
    let projection = Arc::new(LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sidecar").join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        Arc::new(holon_api::ConditionBus::new()),
        clock,
        engine.clone(),
    ));
    projection.install_doc_subscriptions().await?;
    projection.arm();

    insert(&global, "g-0")?;
    insert(&layout, "l-0")?;
    one_pass_feeds(&projection, &engine, &global, &layout, "cold full walk").await?;

    insert(&global, "g-1")?;
    insert(&layout, "l-1")?;
    one_pass_feeds(&projection, &engine, &global, &layout, "incremental").await?;

    let heads = |doc: &LoroDocument| doc.with_read(|d| Ok(d.oplog_frontiers()));
    let (global_before, layout_before) = (heads(&global)?, heads(&layout)?);
    touch_outside_the_tree(&global, 0)?;
    touch_outside_the_tree(&layout, 0)?;
    one_pass_feeds(&projection, &engine, &global, &layout, "no block facts").await?;

    let fed = projection.commit_clock().high_water().get();
    check_out_and_back(&global, &global_before)?;
    check_out_and_back(&layout, &layout_before)?;
    assert!(
        projection.commit_clock().high_water().get() > fed,
        "the checkout round trip minted no stamp"
    );
    one_pass_feeds(&projection, &engine, &global, &layout, "idle").await?;

    let writers: Vec<_> = [global.clone(), layout.clone()]
        .into_iter()
        .enumerate()
        .map(|(w, doc)| {
            std::thread::spawn(move || -> Result<()> {
                for i in 0..40 {
                    insert(&doc, &format!("w{w}-{i}"))?;
                    if i % 7 == 0 {
                        touch_outside_the_tree(&doc, i)?;
                    }
                }
                Ok(())
            })
        })
        .collect();
    for _ in 0..20 {
        projection.project().await?;
        tokio::task::yield_now().await;
    }
    for writer in writers {
        writer.join().unwrap()?;
    }
    settle_and_assert_fed(&projection, &engine, &global, &layout, "concurrent writers").await
}
