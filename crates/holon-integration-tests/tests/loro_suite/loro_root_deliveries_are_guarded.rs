//! Every commit the projected docs' root subscription sees is delivered on the
//! committing thread while it holds the doc's write guard: a commit number
//! minted in that callback is then ordered before the writer returns.
//!
//! @pbt kind harness
//! @pbt covers loro-root-delivery-guarded — no deferred or unguarded root
//! delivery on the global or layout doc

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Result;
use holon_core::OriginTaggedWrites;
use holon_core::cell::TextCellBacking;
use holon_core::cell::TextOp;
use holon_loro::CONTENT_RAW;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::STABLE_ID;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::emit_probe::counts_for;
use holon_loro::loro_text_cell_backing::LoroTextCellBacking;
use loro::Frontiers;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;

const GUARDED: &str = "probe.root_delivery_guarded";
const IMPORTED: &str = "probe.root_delivery_imported";
const RAW: &str = "root_delivery_raw_control";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_root_delivery_holds_the_doc_write_guard() -> Result<()> {
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
        tempdir.path().join("sidecar").join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        Arc::new(holon_api::ConditionBus::new()),
    ));
    projection.install_doc_subscriptions().await?;
    projection.arm();

    // Concurrent writers on both docs while projection passes read them.
    let mut writers = Vec::new();
    for (w, doc) in [
        global.clone(),
        layout.clone(),
        global.clone(),
        layout.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        writers.push(std::thread::spawn(move || -> Result<()> {
            for i in 0..50 {
                doc.with_write(WriteOrigin::Probe("root_delivery_guarded"), |txn| {
                    let tree = txn.get_tree(TREE_NAME);
                    let node = tree.create(None)?;
                    let meta = tree.get_meta(node)?;
                    meta.insert(STABLE_ID, format!("w{w}-{i}"))?;
                    meta.ensure_mergeable_text(CONTENT_RAW)?.insert(0, "x")?;
                    Ok(())
                })?;
            }
            Ok(())
        }));
    }
    let passes = {
        let projection = projection.clone();
        tokio::spawn(async move {
            for _ in 0..40 {
                projection.project().await.unwrap();
                tokio::task::yield_now().await;
            }
        })
    };
    for writer in writers {
        writer.join().unwrap()?;
    }
    passes.await?;

    // A peer delta imported through the guarded import path.
    let peer = loro::LoroDoc::new();
    peer.set_peer_id(77)?;
    let node = peer.get_tree(TREE_NAME).create(None)?;
    peer.get_tree(TREE_NAME)
        .get_meta(node)?
        .insert(STABLE_ID, "peer-0")?;
    peer.commit();
    global.apply_update_with_origin(
        WriteOrigin::Probe("root_delivery_imported"),
        &peer.export(loro::ExportMode::all_updates())?,
    )?;

    // The editor's keystroke, which commits through the cell's own guard.
    let text = global.with_write(WriteOrigin::Probe("root_delivery_guarded"), |txn| {
        let tree = txn.get_tree(TREE_NAME);
        let node = tree.create(None)?;
        Ok(tree.get_meta(node)?.ensure_mergeable_text(CONTENT_RAW)?)
    })?;
    let cell = LoroTextCellBacking::new(global.doc(), text)?;
    let keystrokes_before = counts_for("ui_editor_echo");
    cell.apply_text_op(TextOp::Insert {
        pos_codepoint: 0,
        text: "k".to_string(),
    })?;
    let keystrokes = counts_for("ui_editor_echo");

    // Positive control: a raw commit outside the guard IS counted.
    let raw = global.doc();
    raw.get_tree(TREE_NAME).create(None)?;
    raw.set_next_commit_origin(RAW);
    raw.commit();

    let guarded = counts_for(GUARDED);
    assert_eq!(guarded.unguarded, 0, "with_write deliveries: {guarded:?}");
    assert!(guarded.guarded >= 200, "with_write deliveries: {guarded:?}");
    let imported = counts_for(IMPORTED);
    assert_eq!(
        imported,
        holon_loro::emit_probe::EmitCounts {
            guarded: 1,
            unguarded: 0
        }
    );
    assert_eq!(keystrokes.unguarded, keystrokes_before.unguarded);
    assert_eq!(keystrokes.guarded, keystrokes_before.guarded + 1);
    assert_eq!(
        counts_for(RAW),
        holon_loro::emit_probe::EmitCounts {
            guarded: 0,
            unguarded: 1
        }
    );
    Ok(())
}
