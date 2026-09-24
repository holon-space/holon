//! The projection's settle predicate against the sink it feeds.
//!
//! Settled means every change in BOTH projected documents is in the sink. The
//! sink hook runs mid-apply — after the pass drained the pending facts, before
//! the rows land — which is the window a quiescence poll can hit.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-settle — no settle while a change is in flight
//! or withheld

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
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
        let projection = Arc::new(LoroProjection::new(
            doc_store.clone(),
            Arc::new(StdMutex::new(Frontiers::default())),
            sink.clone() as Arc<dyn OriginTaggedWrites>,
            sink.clone() as Arc<dyn SinkReader>,
            tempdir.path().join("sc.sync"),
            holon_api::block_read_model::BlockReadModel::new(),
            Arc::new(holon_api::ConditionBus::new()),
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
        })
    }

    fn settled(&self) -> bool {
        settled(&self.projection, &self.global, &self.layout)
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
    projection.is_settled_at(
        &global.doc().oplog_frontiers(),
        &layout.doc().oplog_frontiers(),
    )
}

fn insert_child_in(doc: &LoroDocument, parent: TreeID, stable_id: &str) -> Result<()> {
    let doc = doc.doc();
    let tree = doc.get_tree(TREE_NAME);
    let node = tree.create(Some(parent))?;
    let meta = tree.get_meta(node)?;
    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id))?;
    meta.insert(CONTENT_TYPE, loro::LoroValue::from("text"))?;
    meta.ensure_mergeable_text(CONTENT_RAW)?
        .insert(0, stable_id)?;
    doc.commit();
    Ok(())
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
