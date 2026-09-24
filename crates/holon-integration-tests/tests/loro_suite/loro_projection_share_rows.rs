//! The full reseed spares a share's rows only while that share is registered.
//!
//! A share's rows are written by its own projection from its own doc, so the
//! global full walk must not diff them away. What marks a row as a share's is
//! the share registry (the live nodes of every registered shared doc, plus a
//! block share's container), never the `shared-tree-id` property: that property
//! is ordinary drawer data a user's file can carry. Martin's vault holds 111
//! ordinary blocks with a stale stamp (bugfunnel
//! 2026-08-02-three-persistent-unclearable-toasts-naming-none).
//!
//! @pbt kind harness
//! @pbt covers loro-projection-share-rows — a stale `shared-tree-id` stamp, a
//!   pre-D198.a page-share mount row, and the rows of a share that failed to
//!   rehydrate or whose doc lost its root are reconciled away by a full walk

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::share_props::SHARE_ROLE_MOUNT;
use holon_api::share_props::SHARE_ROLE_PROPERTY;
use holon_api::share_props::SHARED_TREE_ID_PROPERTY;
use holon_core::OriginTaggedWrites;
use holon_core::ProjectionPass;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::SinkReader;
use loro::Frontiers;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;
use crate::projection_harness::insert_root_block_in;

fn stamped(id: &str, content: &str, props: &[(&str, &str)]) -> Block {
    let mut block = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), content);
    for (key, value) in props {
        block.set_property(*key, *value);
    }
    block
}

/// A cold-boot projection over a store holding one ordinary block, armed so the
/// full walk may delete.
async fn cold_boot() -> Result<(tempfile::TempDir, Arc<MemorySink>, LoroProjection)> {
    let (dir, _, sink, projection) = cold_boot_with_store().await?;
    Ok((dir, sink, projection))
}

async fn cold_boot_with_store() -> Result<(
    tempfile::TempDir,
    Arc<RwLock<LoroDocumentStore>>,
    Arc<MemorySink>,
    LoroProjection,
)> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    insert_root_block_in(&doc_store, DocScope::Global, "kept", "an ordinary block").await?;
    let sink = Arc::new(MemorySink::new());
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        Arc::new(holon_api::ConditionBus::new()),
    );
    projection.arm();
    Ok((tempdir, doc_store, sink, projection))
}

/// A shared doc holding one root block `stable_id`; `deleted` removes it again,
/// the way the owner deleting the shared page leaves a recipient's copy.
fn shared_doc(stable_id: &str, deleted: bool) -> Result<loro::LoroDoc> {
    let doc = loro::LoroDoc::new();
    let tree = doc.get_tree(holon_loro::TREE_NAME);
    let node = tree.create(None)?;
    let meta = tree.get_meta(node)?;
    meta.insert(holon_loro::STABLE_ID, loro::LoroValue::from(stable_id))?;
    meta.insert(holon_loro::CONTENT_TYPE, loro::LoroValue::from("text"))?;
    meta.ensure_mergeable_text(holon_loro::CONTENT_RAW)?
        .insert(0, "shared content")?;
    doc.commit();
    if deleted {
        tree.delete(node)?;
        doc.commit();
    }
    Ok(doc)
}

#[tokio::test]
async fn a_stale_share_stamp_does_not_protect_a_deleted_block() -> Result<()> {
    let (_dir, sink, projection) = cold_boot().await?;
    sink.plant_row(stamped(
        "stale",
        "a block whose drawer names a share that is gone",
        &[(SHARED_TREE_ID_PROPERTY, "a-share-long-gone")],
    ));
    assert!(
        sink.read_blocks().await?["block:stale"]
            .block
            .properties_map()
            .contains_key(SHARED_TREE_ID_PROPERTY),
        "precondition: the planted row carries the stamp"
    );

    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.row_ids(),
        ["block:kept"],
        "the full walk must delete a row Loro does not hold, whatever its drawer says"
    );
    Ok(())
}

#[tokio::test]
async fn a_pre_overlay_page_share_mount_row_is_reconciled_away() -> Result<()> {
    let (_dir, sink, projection) = cold_boot().await?;
    sink.plant_row(stamped(
        "old-mount",
        "My Shared Page",
        &[
            (SHARED_TREE_ID_PROPERTY, "old-share"),
            (SHARE_ROLE_PROPERTY, SHARE_ROLE_MOUNT),
        ],
    ));

    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.row_ids(),
        ["block:kept"],
        "no registered share owns the old mount row, so the full walk deletes it"
    );
    Ok(())
}

#[tokio::test]
async fn a_loaded_shares_rows_and_container_survive_the_full_walk() -> Result<()> {
    let (_dir, doc_store, sink, projection) = cold_boot_with_store().await?;
    let mut store = holon_loro::shared_tree::InMemorySharedTreeStore::new();
    store.insert("live-share".into(), shared_doc("shared-live", false)?);
    let projection = projection.with_shared_trees(Arc::new(store));
    {
        let global = doc_store.read().await.get_doc(DocScope::Global).await?;
        let doc = global.doc();
        let tree = doc.get_tree(holon_loro::TREE_NAME);
        let mount = holon_loro::shared_tree::create_mount_node(
            &tree,
            None,
            "live-share",
            loro::TreeID::new(1, 0),
        )?;
        tree.get_meta(mount)?
            .insert(holon_loro::STABLE_ID, loro::LoroValue::from("container"))?;
        holon_loro::shared_tree::record_mount(
            &tree,
            mount,
            &holon_loro::shared_tree::ShareKind::Block,
            holon_loro::shared_tree::MountRole::Recipient,
        )?;
        doc.commit();
    }
    sink.plant_row(stamped(
        "shared-live",
        "a block of a loaded share",
        &[(SHARED_TREE_ID_PROPERTY, "live-share")],
    ));
    sink.plant_row(stamped(
        "container",
        "Shared tree (live-share)",
        &[
            (SHARED_TREE_ID_PROPERTY, "live-share"),
            (SHARE_ROLE_PROPERTY, SHARE_ROLE_MOUNT),
        ],
    ));

    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.row_ids(),
        ["block:container", "block:kept", "block:shared-live"],
        "a loaded share's rows and its container are the share projection's, not this walk's"
    );
    Ok(())
}

#[tokio::test]
async fn the_rows_of_a_share_whose_doc_lost_its_root_are_reconciled_away() -> Result<()> {
    let (_dir, sink, projection) = cold_boot().await?;
    let mut store = holon_loro::shared_tree::InMemorySharedTreeStore::new();
    store.insert("dead-share".into(), shared_doc("shared-dead", true)?);
    let projection = projection.with_shared_trees(Arc::new(store));
    sink.plant_row(stamped(
        "shared-dead",
        "the page the owner deleted",
        &[(SHARED_TREE_ID_PROPERTY, "dead-share")],
    ));

    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.row_ids(),
        ["block:kept"],
        "a loaded share no longer holds the node, so nothing protects its row"
    );
    Ok(())
}

/// Rehydration skips a share it cannot load (the `RehydrationFailed` path):
/// the mount stays in the global tree, but no shared doc is registered, so
/// neither the container nor the share's blocks are any share projection's.
#[tokio::test]
async fn the_rows_of_a_share_that_failed_to_rehydrate_are_reconciled_away() -> Result<()> {
    let (_dir, doc_store, sink, projection) = cold_boot_with_store().await?;
    let projection = projection.with_shared_trees(Arc::new(
        holon_loro::shared_tree::InMemorySharedTreeStore::new(),
    ));
    {
        let global = doc_store.read().await.get_doc(DocScope::Global).await?;
        let doc = global.doc();
        let tree = doc.get_tree(holon_loro::TREE_NAME);
        let mount = holon_loro::shared_tree::create_mount_node(
            &tree,
            None,
            "unloaded-share",
            loro::TreeID::new(1, 0),
        )?;
        tree.get_meta(mount)?
            .insert(holon_loro::STABLE_ID, loro::LoroValue::from("container"))?;
        holon_loro::shared_tree::record_mount(
            &tree,
            mount,
            &holon_loro::shared_tree::ShareKind::Block,
            holon_loro::shared_tree::MountRole::Recipient,
        )?;
        doc.commit();
    }
    sink.plant_row(stamped(
        "container",
        "Shared tree (unloaded-share)",
        &[
            (SHARED_TREE_ID_PROPERTY, "unloaded-share"),
            (SHARE_ROLE_PROPERTY, SHARE_ROLE_MOUNT),
        ],
    ));
    sink.plant_row(stamped(
        "shared-unloaded",
        "a block of the share that did not load",
        &[(SHARED_TREE_ID_PROPERTY, "unloaded-share")],
    ));

    assert_eq!(projection.project().await?, ProjectionPass::Converged);

    assert_eq!(
        sink.row_ids(),
        ["block:kept"],
        "a share that is mounted but not loaded owns no rows, so the full walk deletes them"
    );
    Ok(())
}
