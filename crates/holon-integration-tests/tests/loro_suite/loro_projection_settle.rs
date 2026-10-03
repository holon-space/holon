//! The projection's settle predicate against the sink it feeds.
//!
//! Settled means every change in BOTH projected documents is in the sink. The
//! sink hook runs mid-apply — after the pass drained the pending facts, before
//! the rows land — which is the window a quiescence poll can hit.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-settle — no settle while a change is in flight
//! or owed

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_api::commit_clock::CommitSource;
use holon_core::OriginTaggedWrites;
use holon_core::ProjectionPass;
use holon_loro::CONTENT_RAW;
use holon_loro::CONTENT_TYPE;
use holon_loro::DocScope;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::STABLE_ID;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use loro::Frontiers;
use loro::TreeID;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;
use crate::projection_harness::delete_block_in;
use crate::projection_harness::insert_root_block;
use crate::projection_harness::insert_root_block_in;

struct Fixture {
    _tempdir: tempfile::TempDir,
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    global: Arc<LoroDocument>,
    layout: Arc<LoroDocument>,
    sink: Arc<MemorySink>,
    projection: Arc<LoroProjection>,
    degraded: Arc<holon_api::ConditionBus>,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
            tempdir.path().to_path_buf(),
        )));
        let global = doc_store.read().await.get_doc(DocScope::Global).await?;
        let layout = doc_store.read().await.get_doc(DocScope::Layout).await?;
        let sink = Arc::new(MemorySink::new());
        let degraded = Arc::new(holon_api::ConditionBus::new());
        let projection = Arc::new(LoroProjection::new(
            doc_store.clone(),
            Arc::new(StdMutex::new(Frontiers::default())),
            sink.clone() as Arc<dyn OriginTaggedWrites>,
            sink.clone() as Arc<dyn SinkReader>,
            tempdir.path().join("sidecar").join("sc.sync"),
            holon_api::block_read_model::BlockReadModel::new(),
            degraded.clone(),
        ));
        projection.install_doc_subscriptions().await?;
        projection.arm();
        Ok(Self {
            _tempdir: tempdir,
            doc_store,
            global,
            layout,
            sink,
            projection,
            degraded,
        })
    }

    fn settled(&self) -> bool {
        settled(&self.projection, &self.global, &self.layout)
    }

    fn set_sidecar_dir_mode(&self, mode: u32) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            self._tempdir.path().join("sidecar"),
            std::fs::Permissions::from_mode(mode),
        )?;
        Ok(())
    }

    /// Record the settle verdict at the moment the sink write starts.
    fn observe_settle_mid_apply(&self) -> Arc<StdMutex<Vec<bool>>> {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let (projection, global, layout, out) = (
            self.projection.clone(),
            self.global.clone(),
            self.layout.clone(),
            seen.clone(),
        );
        self.sink.set_apply_hook(Arc::new(move || {
            out.lock()
                .unwrap()
                .push(settled(&projection, &global, &layout));
        }));
        seen
    }
}

fn settled(projection: &LoroProjection, global: &LoroDocument, layout: &LoroDocument) -> bool {
    let frontiers = |doc: &LoroDocument| {
        doc.with_read(|d| Ok(d.oplog_frontiers()))
            .expect("reading the oplog frontiers under the read guard")
    };
    projection.is_settled_at(&frontiers(global), &frontiers(layout))
}

fn insert_child_in(doc: &LoroDocument, parent: TreeID, stable_id: &str) -> Result<()> {
    doc.with_write(holon_loro::WriteOrigin::Probe("settle"), |txn| {
        let tree = txn.get_tree(TREE_NAME);
        let node = tree.create(Some(parent))?;
        let meta = tree.get_meta(node)?;
        meta.insert(STABLE_ID, loro::LoroValue::from(stable_id))?;
        meta.insert(CONTENT_TYPE, loro::LoroValue::from("text"))?;
        meta.ensure_mergeable_text(CONTENT_RAW)?
            .insert(0, stable_id)?;
        txn.commit();
        Ok(())
    })
}

#[tokio::test]
async fn a_layout_create_is_not_settled_before_its_row_lands() -> Result<()> {
    let fx = Fixture::new().await?;
    insert_root_block(&fx.doc_store, "global-id", "global").await?;
    let parent = insert_root_block_in(&fx.doc_store, DocScope::Layout, "panel-id", "panel").await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);
    assert!(fx.settled(), "the seed pass settles both documents");

    insert_child_in(&fx.layout, parent, "panel-child-id")?;
    assert!(
        !fx.settled(),
        "a committed layout create is owed to the sink"
    );

    let mid_apply = fx.observe_settle_mid_apply();
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);
    assert_eq!(
        *mid_apply.lock().unwrap(),
        [false],
        "the layout create's pass reported settled while its row was still in flight"
    );
    assert!(
        fx.sink
            .row_ids()
            .contains(&"block:panel-child-id".to_string()),
        "the create reached the sink: {:?}",
        fx.sink.row_ids()
    );
    assert!(fx.settled(), "settled once the row landed");
    Ok(())
}

#[tokio::test]
async fn a_pass_that_owes_a_withheld_op_is_not_settled() -> Result<()> {
    let fx = Fixture::new().await?;
    insert_root_block(&fx.doc_store, "keep-id", "kept").await?;
    let gone = insert_root_block(&fx.doc_store, "gone-id", "removed later").await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);

    delete_block_in(&fx.doc_store, DocScope::Global, gone).await?;
    let half_born = fx
        .global
        .with_write(holon_loro::WriteOrigin::Probe("settle"), |txn| {
            Ok(txn.get_tree(TREE_NAME).create(None)?)
        })?;
    assert_eq!(
        fx.projection.project().await?,
        ProjectionPass::Incomplete { withheld: 1 }
    );
    assert!(
        !fx.settled(),
        "the withheld delete is still owed to the sink, so the projection is not settled"
    );

    delete_block_in(&fx.doc_store, DocScope::Global, half_born).await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);
    assert_eq!(fx.sink.row_ids(), ["block:keep-id"]);
    assert!(fx.settled());
    Ok(())
}

fn model_holds(fx: &Fixture, id: &str) -> bool {
    use holon_api::block_read_model::BlockDeltaSource;
    fx.projection
        .read_model()
        .as_ref()
        .blocks()
        .read()
        .contains_key(id)
}

/// A snapshot with an unreadable live node is not published to the read
/// model, so the commits it walked stay unfed until a settled pass.
#[tokio::test]
async fn a_pass_over_an_unsettled_snapshot_reports_unsettled_until_the_node_settles() -> Result<()>
{
    let fx = Fixture::new().await?;
    insert_root_block(&fx.doc_store, "seed-id", "seed").await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);

    let half_born = fx
        .global
        .with_write(holon_loro::WriteOrigin::Probe("settle"), |txn| {
            Ok(txn.get_tree(TREE_NAME).create(None)?)
        })?;
    insert_root_block(&fx.doc_store, "late-id", "late").await?;
    for _ in 0..2 {
        let clock = fx.projection.commit_clock();
        assert_eq!(
            fx.projection.project().await?,
            ProjectionPass::Unsettled,
            "a Converged pass ends the run loop's re-drive while global stamps {:?} and layout \
             stamps {:?} are unfed",
            clock.outstanding(CommitSource::LoroGlobal),
            clock.outstanding(CommitSource::LoroLayout),
        );
        assert!(
            !fx.settled() || model_holds(&fx, "block:late-id"),
            "settled while the read model lacks block:late-id"
        );
    }

    fx.global
        .with_write(holon_loro::WriteOrigin::Probe("settle"), |txn| {
            holon_loro::write_stable_id(txn, half_born, "grown-id")?;
            let meta = txn.get_tree(TREE_NAME).get_meta(half_born)?;
            meta.insert(CONTENT_TYPE, loro::LoroValue::from("text"))?;
            meta.ensure_mergeable_text(CONTENT_RAW)?
                .insert(0, "grown")?;
            Ok(())
        })?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);
    assert!(fx.settled());
    assert!(model_holds(&fx, "block:late-id") && model_holds(&fx, "block:grown-id"));
    Ok(())
}

/// A flush that gives up on an unsettled pass discloses it through the
/// projection, which must raise the banner the run loop's next converged pass
/// clears.
#[tokio::test]
async fn a_disclosed_unsettled_flush_raises_the_run_loops_banner() -> Result<()> {
    use holon_core::DownstreamProjection;

    let fx = Fixture::new().await?;
    fx.global
        .with_write(holon_loro::WriteOrigin::Probe("settle"), |txn| {
            Ok(txn.get_tree(TREE_NAME).create(None)?)
        })?;
    assert_eq!(
        DownstreamProjection::flush(fx.projection.as_ref())
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        ProjectionPass::Unsettled
    );
    fx.projection
        .disclose_degraded(ProjectionPass::UNSETTLED_REASON.to_string());

    let current = fx.degraded.current();
    assert_eq!(current.len(), 1, "{current:?}");
    assert_eq!(
        current[0].condition_key(),
        holon_loro::loro_sync_controller::projection_degraded_key()
    );
    assert!(
        format!("{:?}", current[0].reason).contains(ProjectionPass::UNSETTLED_REASON),
        "{current:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_pass_whose_sidecar_write_fails_is_not_settled_and_recovers() -> Result<()> {
    let fx = Fixture::new().await?;
    insert_root_block(&fx.doc_store, "a-id", "a").await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);

    let b = insert_root_block(&fx.doc_store, "b-id", "b").await?;
    fx.set_sidecar_dir_mode(0o500)?;
    let failed = fx.projection.project().await;
    fx.set_sidecar_dir_mode(0o700)?;
    let err = failed.expect_err("an unwritable sidecar fails the pass");
    assert!(format!("{err:#}").contains("sidecar"), "{err:#}");
    assert!(!fx.settled(), "a pass that returned Err reported settled");

    delete_block_in(&fx.doc_store, DocScope::Global, b).await?;
    assert_eq!(fx.projection.project().await?, ProjectionPass::Converged);
    assert_eq!(
        fx.sink.row_ids(),
        ["block:a-id"],
        "the delete after the failed pass never reached the sink"
    );
    assert!(fx.settled());
    Ok(())
}
