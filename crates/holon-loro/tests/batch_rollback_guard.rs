//! The guarded rollback the block write authority offers a failed multi-op
//! batch: it undoes a window the batch alone wrote, across every document a
//! block write can reach, and refuses one it cannot undo without destroying
//! something.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::repository::CoreOperations;
use holon_api::repository::Traversal;
use holon_core::BatchRollback;
use holon_core::BatchWindow;
use holon_core::RollbackRefused;
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document_store::DocScope;
use tokio::sync::RwLock;

const OUR_PEER: u64 = 7;
const THEIR_PEER: u64 = 8;

fn store_at(dir: &std::path::Path, peer: u64) -> LoroDocumentStore {
    LoroDocumentStore::new(dir.to_path_buf()).with_peer_id(Some(peer))
}

fn ops_over(store: &LoroDocumentStore) -> LoroBlockOperations {
    LoroBlockOperations::new(Arc::new(RwLock::new(store.clone())))
}

async fn backend_for(store: &LoroDocumentStore, scope: DocScope) -> LoroBackend {
    LoroBackend::from_document(store.get_doc(scope).await.expect("document"))
}

async fn create(backend: &LoroBackend, parent: EntityUri, id: &str) {
    backend
        .create_block_with_properties(
            parent,
            BlockContent::text(id),
            Some(EntityUri::block(id)),
            &HashMap::new(),
            &BlockEdges::default(),
        )
        .await
        .unwrap();
}

async fn live_ids(backend: &LoroBackend) -> Vec<String> {
    let mut ids: Vec<String> = backend
        .get_all_blocks(Traversal::ALL)
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.id.to_string())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_window_the_batch_alone_wrote_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;
    let before = live_ids(&backend).await;

    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&backend, EntityUri::block("root"), "op1").await;
    window.absorb(ops.observe().await.unwrap());
    assert_eq!(
        live_ids(&backend).await.len(),
        before.len() + 1,
        "op 1 must be in the store before the rollback"
    );

    ops.rollback_to(&window).await.unwrap();

    assert_eq!(
        live_ids(&backend).await,
        before,
        "the rollback must restore the pre-batch block set"
    );
}

/// A write this session made that is not the batch's. It carries no batch
/// identity, so the only thing that separates it is the interval between two
/// of the batch's ops, in which the batch writes nothing.
#[tokio::test]
async fn a_local_write_between_two_batch_ops_refuses_the_rollback_and_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;

    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&backend, EntityUri::block("root"), "ours").await;
    window.absorb(ops.observe().await.unwrap());

    create(&backend, EntityUri::block("root"), "uiedit").await;
    window.observe_between_ops(ops.observe().await.unwrap());

    let refusal = ops.rollback_to(&window).await.expect_err("must refuse");
    assert!(
        matches!(&refusal, RollbackRefused::LocalWroteInsideWindow { doc, .. } if doc == "vault"),
        "the refusal must name the document the intruder wrote to: {refusal:?}"
    );

    let ids = live_ids(&backend).await;
    assert!(
        ids.iter().any(|i| i.contains("uiedit")),
        "a refused rollback must leave this session's other write in place: {ids:?}"
    );
    assert!(
        ids.iter().any(|i| i.contains("ours")),
        "a refused rollback must leave the batch's own ops in place too: {ids:?}"
    );
}

#[tokio::test]
async fn a_peers_write_inside_the_window_refuses_the_rollback_and_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let backend = LoroBackend::from_document(doc.clone());
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;

    let peer_doc =
        Arc::new(LoroDocument::new_with_peer_id("peer".to_string(), Some(THEIR_PEER)).unwrap());
    peer_doc
        .apply_update_with_origin(WriteOrigin::SyncImport, &doc.export_snapshot().unwrap())
        .unwrap();

    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&backend, EntityUri::block("root"), "ours").await;

    let peer_backend = LoroBackend::from_document(peer_doc.clone());
    create(&peer_backend, EntityUri::block("root"), "theirs").await;
    doc.apply_update_with_origin(
        WriteOrigin::SyncImport,
        &peer_doc.export_snapshot().unwrap(),
    )
    .unwrap();
    window.absorb(ops.observe().await.unwrap());

    let refusal = ops.rollback_to(&window).await.expect_err("must refuse");
    assert!(
        matches!(
            &refusal,
            RollbackRefused::PeerWroteInsideWindow { doc, peer: THEIR_PEER, .. } if doc == "vault"
        ),
        "the refusal must name the peer that wrote and the document: {refusal:?}"
    );

    let ids = live_ids(&backend).await;
    assert!(
        ids.iter().any(|i| i.contains("theirs")),
        "a refused rollback must leave the peer's block in the store: {ids:?}"
    );
    assert!(
        ids.iter().any(|i| i.contains("ours")),
        "a refused rollback must leave our own ops in place too: {ids:?}"
    );
}

/// `LoroBackend::resolve_write_target_sync` probes the device-local layout
/// document before the vault one, so a batch can write there and a rollback
/// that measured only the vault document would report a rewind it did not
/// perform.
#[tokio::test]
async fn a_batch_write_into_the_layout_document_is_rolled_back_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let vault = backend_for(&store, DocScope::Global).await;
    let layout = backend_for(&store, DocScope::Layout).await;
    let ops = ops_over(&store);

    create(&vault, EntityUri::no_parent(), "root").await;
    create(&layout, EntityUri::no_parent(), "layoutroot").await;
    let vault_before = live_ids(&vault).await;
    let layout_before = live_ids(&layout).await;

    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&vault, EntityUri::block("root"), "vaultop").await;
    create(&layout, EntityUri::block("layoutroot"), "layoutop").await;
    window.absorb(ops.observe().await.unwrap());
    assert!(
        live_ids(&layout)
            .await
            .iter()
            .any(|i| i.contains("layoutop")),
        "the layout write must be in the store before the rollback"
    );

    ops.rollback_to(&window).await.unwrap();

    assert_eq!(live_ids(&vault).await, vault_before);
    assert_eq!(
        live_ids(&layout).await,
        layout_before,
        "a rollback that reports a full rewind must include the layout document"
    );
}

#[tokio::test]
async fn an_intruding_layout_write_refuses_the_rollback_and_names_that_document() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let vault = backend_for(&store, DocScope::Global).await;
    let layout = backend_for(&store, DocScope::Layout).await;
    let ops = ops_over(&store);

    create(&vault, EntityUri::no_parent(), "root").await;
    create(&layout, EntityUri::no_parent(), "layoutroot").await;

    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&vault, EntityUri::block("root"), "vaultop").await;
    window.absorb(ops.observe().await.unwrap());

    create(&layout, EntityUri::block("layoutroot"), "intruder").await;
    window.observe_between_ops(ops.observe().await.unwrap());

    let refusal = ops.rollback_to(&window).await.expect_err("must refuse");
    assert!(
        matches!(&refusal, RollbackRefused::LocalWroteInsideWindow { doc, .. } if doc == "layout"),
        "the refusal must name the layout document: {refusal:?}"
    );
    assert!(
        live_ids(&layout)
            .await
            .iter()
            .any(|i| i.contains("intruder")),
        "a refused rollback must leave the layout write in place"
    );
}

/// Periodic history compaction (`LoroDocumentStore::save_all`) writes a shallow
/// snapshot, so a window opened before the trim no longer names a version the
/// reloaded document can reconstruct.
#[tokio::test]
async fn a_window_opened_before_the_shallow_root_refuses_the_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;
    let mut window = BatchWindow::opened(ops.observe().await.unwrap());
    create(&backend, EntityUri::block("root"), "op1").await;
    window.absorb(ops.observe().await.unwrap());
    store.save_all().await.unwrap();

    let reloaded = store_at(dir.path(), OUR_PEER);
    let reloaded_backend = backend_for(&reloaded, DocScope::Global).await;
    let before = live_ids(&reloaded_backend).await;

    let refusal = ops_over(&reloaded)
        .rollback_to(&window)
        .await
        .expect_err("must refuse");
    assert!(
        matches!(&refusal, RollbackRefused::HistoryTrimmed { doc } if doc == "vault"),
        "a trimmed history must refuse typed, not silently no-op: {refusal:?}"
    );
    assert_eq!(
        live_ids(&reloaded_backend).await,
        before,
        "a refused rollback must change nothing"
    );
}
