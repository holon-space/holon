//! The rollback the block write authority offers a failed multi-op batch: it
//! undoes the writes made inside the batch's scope, across every document a
//! block write can reach, and keeps every write that was not the batch's.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::repository::CoreOperations;
use holon_api::repository::Traversal;
use holon_core::BatchRollback;
use holon_core::RollbackRefused;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::cell_registry::EntityCellRegistryExt as _;
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::WriteOrigin;
use holon_loro::block_cell_registry::BlockCellRegistry;
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

async fn text_of(backend: &LoroBackend, id: &str) -> String {
    backend
        .get_block(&format!("block:{id}"))
        .await
        .unwrap()
        .content_text()
        .to_string()
}

/// A keystroke at the start of `id`'s text, through the editor's cell on the
/// editor's own thread.
async fn type_into(store: &LoroDocumentStore, id: &str, text: &str) {
    let global = store.get_doc(DocScope::Global).await.unwrap();
    let layout = store.get_doc(DocScope::Layout).await.unwrap();
    let (uri, text) = (EntityUri::block(id), text.to_string());
    std::thread::spawn(move || {
        let registry =
            BlockCellRegistry::with_loro(global, layout, Arc::new(holon_core::NoReadOnlyDocuments));
        let registry: &dyn EntityCellRegistry = &registry;
        registry
            .editable_field::<String>(&uri, "content")
            .unwrap()
            .apply_text_op(holon_core::cell::TextOp::Insert {
                pos_codepoint: 0,
                text,
            })
            .unwrap();
    })
    .join()
    .expect("the editor thread panicked");
}

#[tokio::test]
async fn a_batch_alone_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;
    let before = live_ids(&backend).await;

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(create(&backend, EntityUri::block("root"), "op1"))
        .await;
    assert_eq!(live_ids(&backend).await.len(), before.len() + 1);

    let rolled = batch.roll_back().await.unwrap();

    assert_eq!(live_ids(&backend).await, before);
    assert!(rolled.undone_steps >= 1, "{rolled:?}");
    assert!(rolled.lost.is_empty(), "{rolled:?}");
}

/// A block op outside the batch's scope commits under the plain block-op
/// origin, so the batch's undo history does not hold it.
#[tokio::test]
async fn a_write_outside_the_batch_scope_between_two_batch_ops_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;

    let batch = ops.open().await.unwrap();
    let id = batch.id();
    id.scope(create(&backend, EntityUri::block("root"), "ours1"))
        .await;
    create(&backend, EntityUri::block("root"), "uiedit").await;
    id.scope(create(&backend, EntityUri::block("root"), "ours2"))
        .await;

    let rolled = batch.roll_back().await.unwrap();

    assert_eq!(live_ids(&backend).await, vec!["block:root", "block:uiedit"]);
    assert!(rolled.lost.is_empty(), "{rolled:?}");
}

/// A peer's import inside the window, including a text edit on the block the
/// batch edits, survives the undo, and the undo replicates to the peer.
#[tokio::test]
async fn a_peers_write_inside_the_window_survives_and_the_peer_converges() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let backend = LoroBackend::from_document(doc.clone());
    let ops = ops_over(&store);

    create(&backend, EntityUri::no_parent(), "root").await;
    create(&backend, EntityUri::block("root"), "a").await;

    let peer_doc =
        Arc::new(LoroDocument::new_with_peer_id("peer".to_string(), Some(THEIR_PEER)).unwrap());
    let sync = |from: &LoroDocument, to: &LoroDocument| {
        to.apply_update_with_origin(WriteOrigin::SyncImport, &from.export_snapshot().unwrap())
            .unwrap();
    };
    sync(&doc, &peer_doc);
    let peer_backend = LoroBackend::from_document(peer_doc.clone());

    let batch = ops.open().await.unwrap();
    let id = batch.id();
    id.scope(async {
        backend.insert_text("block:a", 1, " OURS").await.unwrap();
        create(&backend, EntityUri::block("root"), "ours").await;
    })
    .await;
    peer_backend
        .insert_text("block:a", 0, "theirs:")
        .await
        .unwrap();
    create(&peer_backend, EntityUri::block("root"), "theirs").await;
    sync(&peer_doc, &doc);

    batch.roll_back().await.unwrap();

    assert_eq!(
        live_ids(&backend).await,
        vec!["block:a", "block:root", "block:theirs"]
    );
    assert_eq!(text_of(&backend, "a").await, "theirs:a");
    sync(&doc, &peer_doc);
    assert_eq!(live_ids(&peer_backend).await, live_ids(&backend).await);
    assert_eq!(text_of(&peer_backend, "a").await, "theirs:a");
}

/// `LoroBackend::resolve_write_target_sync` probes the device-local layout
/// document before the vault one, so a batch can write there.
#[tokio::test]
async fn a_batch_write_into_the_layout_document_is_rolled_back_and_a_layout_write_outside_survives()
{
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let vault = backend_for(&store, DocScope::Global).await;
    let layout = backend_for(&store, DocScope::Layout).await;
    let ops = ops_over(&store);

    create(&vault, EntityUri::no_parent(), "root").await;
    create(&layout, EntityUri::no_parent(), "layoutroot").await;
    let vault_before = live_ids(&vault).await;

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(async {
            create(&vault, EntityUri::block("root"), "vaultop").await;
            create(&layout, EntityUri::block("layoutroot"), "layoutop").await;
        })
        .await;
    create(&layout, EntityUri::block("layoutroot"), "outside").await;

    batch.roll_back().await.unwrap();

    assert_eq!(live_ids(&vault).await, vault_before);
    assert_eq!(
        live_ids(&layout).await,
        vec!["block:layoutroot", "block:outside"]
    );
}

/// Two batches open at once, each on its own task, writing interleaved: each
/// rollback takes back exactly its own batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_batches_each_roll_back_only_their_own_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = Arc::new(backend_for(&store, DocScope::Global).await);
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;

    let batch_a = ops.open().await.unwrap();
    let batch_b = ops.open().await.unwrap();
    assert_ne!(batch_a.id(), batch_b.id());
    let turn = Arc::new(tokio::sync::Barrier::new(2));
    let run = |id: holon_core::BatchId, first: &'static str, second: &'static str| {
        let (backend, turn) = (backend.clone(), turn.clone());
        tokio::spawn(id.scope(async move {
            create(&backend, EntityUri::block("root"), first).await;
            turn.wait().await;
            create(&backend, EntityUri::block("root"), second).await;
        }))
    };
    let a = run(batch_a.id(), "a1", "a2");
    let b = run(batch_b.id(), "b1", "b2");
    a.await.unwrap();
    b.await.unwrap();

    batch_a.roll_back().await.unwrap();
    assert_eq!(
        live_ids(&backend).await,
        vec!["block:b1", "block:b2", "block:root"]
    );
    batch_b.roll_back().await.unwrap();
    assert_eq!(live_ids(&backend).await, vec!["block:root"]);
}

/// A checkout clears an undo manager's stacks without a signal. A rollback
/// over a cleared history would undo nothing and report success, so it is
/// refused, and nothing is undone.
#[tokio::test]
async fn a_checkout_inside_the_window_refuses_the_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let backend = LoroBackend::from_document(doc.clone());
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;

    let before = doc.with_read(|d| Ok(d.oplog_frontiers())).unwrap();

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(create(&backend, EntityUri::block("root"), "ours"))
        .await;
    doc.with_write(WriteOrigin::Probe("checkout"), |txn| {
        txn.checkout(&before)?;
        txn.checkout_to_latest();
        Ok(())
    })
    .unwrap();

    let refusal = batch.roll_back().await.expect_err("must refuse");
    assert!(
        matches!(&refusal, RollbackRefused::Unrecorded { doc, .. } if doc == "vault"),
        "{refusal:?}"
    );
    assert_eq!(live_ids(&backend).await, vec!["block:ours", "block:root"]);
}

/// A checkout before the batch's first write leaves the undo history unable
/// to record that write, while later ones are recorded: undoing what is
/// there would roll back part of the batch and report success.
#[tokio::test]
async fn a_checkout_before_the_batch_writes_refuses_the_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let backend = LoroBackend::from_document(doc.clone());
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;
    let before = doc.with_read(|d| Ok(d.oplog_frontiers())).unwrap();
    create(&backend, EntityUri::block("root"), "seed").await;

    let batch = ops.open().await.unwrap();
    doc.with_write(WriteOrigin::Probe("checkout"), |txn| {
        txn.checkout(&before)?;
        txn.checkout_to_latest();
        Ok(())
    })
    .unwrap();
    batch
        .id()
        .scope(async {
            create(&backend, EntityUri::block("root"), "ours1").await;
            create(&backend, EntityUri::block("root"), "ours2").await;
        })
        .await;

    let refusal = batch.roll_back().await.expect_err("must refuse");
    assert!(
        matches!(&refusal, RollbackRefused::Unrecorded { doc, .. } if doc == "vault"),
        "{refusal:?}"
    );
    assert_eq!(
        live_ids(&backend).await,
        vec!["block:ours1", "block:ours2", "block:root", "block:seed"]
    );
}

/// Saving the store compacts what is on disk, not the live document's
/// history, so a save inside the window does not cut the undo short.
#[tokio::test]
async fn a_save_inside_the_window_does_not_cut_the_rollback_short() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(create(&backend, EntityUri::block("root"), "op1"))
        .await;
    store.save_all().await.unwrap();

    batch.roll_back().await.unwrap();
    assert_eq!(live_ids(&backend).await, vec!["block:root"]);
}

/// Keystrokes on the block the batch rewrites and on another block, landing
/// between the batch's commits: the undo takes back only the batch's text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keystrokes_between_batch_commits_survive_on_both_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;
    create(&backend, EntityUri::block("root"), "a").await;
    create(&backend, EntityUri::block("root"), "b").await;

    let batch = ops.open().await.unwrap();
    let id = batch.id();
    id.scope(backend.insert_text("block:a", 1, " BATCH1"))
        .await
        .unwrap();
    type_into(&store, "b", "kb:").await;
    type_into(&store, "a", "ka:").await;
    id.scope(backend.insert_text("block:a", 0, "BATCH2 "))
        .await
        .unwrap();

    batch.roll_back().await.unwrap();

    assert_eq!(text_of(&backend, "a").await, "ka:a");
    assert_eq!(text_of(&backend, "b").await, "kb:b");
}

/// One batch op that commits twice (text, then a property), with a keystroke
/// on the same text between the two commits. The keystroke stays; both of
/// the op's commits go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keystroke_between_two_commits_of_one_batch_op_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;
    create(&backend, EntityUri::block("root"), "a").await;

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(async {
            backend.insert_text("block:a", 1, " OP").await.unwrap();
            type_into(&store, "a", "k:").await;
            backend
                .update_block_properties(
                    "block:a",
                    &HashMap::from([("op".to_string(), Value::String("second".to_string()))]),
                )
                .await
                .unwrap();
        })
        .await;
    type_into(&store, "a", "x").await;

    batch.roll_back().await.unwrap();

    let a = backend.get_block("block:a").await.unwrap();
    assert_eq!(a.content_text(), "xk:a");
    assert!(!a.properties.contains_key("op"), "{:?}", a.properties);
}

/// A keystroke into a block the batch created cannot outlive the undo of that
/// create; the rollback names the block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keystroke_into_a_block_the_batch_created_is_reported_lost() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path(), OUR_PEER);
    let backend = backend_for(&store, DocScope::Global).await;
    let ops = ops_over(&store);
    create(&backend, EntityUri::no_parent(), "root").await;

    let batch = ops.open().await.unwrap();
    batch
        .id()
        .scope(create(&backend, EntityUri::block("root"), "ours"))
        .await;
    type_into(&store, "ours", "typed:").await;

    let rolled = batch.roll_back().await.unwrap();

    assert_eq!(live_ids(&backend).await, vec!["block:root"]);
    assert_eq!(
        rolled.lost,
        vec![holon_core::LostWrite {
            doc: "vault".to_string(),
            block: "block:ours".to_string(),
        }]
    );
}
