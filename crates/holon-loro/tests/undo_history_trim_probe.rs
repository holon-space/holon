//! Measurement: does history compaction empty a Loro `UndoManager`?
//!
//! `LoroDocument::export_compact_snapshot` exports a SHALLOW snapshot — state
//! plus no operation history before the current frontier. The cell-undo lane
//! wants that same document to carry a text `UndoManager`, which walks
//! operation history to build its inverse. The two policies may not be
//! compatible, and the answer decides whether the lane can keep compaction as
//! it is. This file records the measurement rather than arguing about it.

use anyhow::Result;
use holon_loro::LoroDocument;
use holon_loro::WriteOrigin;
use loro::UndoManager;

fn typed_doc(doc_id: &str) -> Result<LoroDocument> {
    let doc = LoroDocument::new(doc_id.to_string())?;
    doc.with_write(WriteOrigin::Probe("trim_probe_seed"), |d| {
        d.get_text("content").insert(0, "seed ")?;
        Ok(())
    })?;
    Ok(doc)
}

/// The in-process question: a manager that could undo before the export — can
/// it still undo after it?
#[test]
fn compaction_leaves_an_in_process_undo_manager_able_to_undo() -> Result<()> {
    let doc = typed_doc("trim-probe-in-process")?;
    // ALLOW(loro_doc_escape): the UndoManager is a long-lived observer that
    // registers on the doc, the same shape as a subscription.
    let raw = doc.doc();
    let mut undo = UndoManager::new(&raw);

    doc.with_write(WriteOrigin::Probe("trim_probe_typing"), |d| {
        d.get_text("content").insert(5, "typed")?;
        Ok(())
    })?;
    undo.record_new_checkpoint()?;
    assert_eq!(doc.get_text("content")?, "seed typed");
    assert!(undo.can_undo(), "the manager must see the typing at all");

    let bytes = doc.export_compact_snapshot()?;
    assert!(!bytes.is_empty(), "the compact export produced nothing");

    assert!(
        undo.can_undo(),
        "history compaction emptied the live undo manager"
    );
    undo.undo()?;
    assert_eq!(
        doc.get_text("content")?,
        "seed ",
        "the undo after compaction did not take the typing back"
    );
    Ok(())
}

/// The restart question: a compacted snapshot reloaded into a fresh document
/// with a fresh manager. Undo cannot survive this, and the lane's ratified
/// policy (D116.a) is that it does not have to — this test states the fact so
/// a later change to compaction cannot quietly move it.
#[test]
fn a_reloaded_compact_snapshot_starts_with_an_empty_undo_stack() -> Result<()> {
    let doc = typed_doc("trim-probe-reload")?;
    doc.with_write(WriteOrigin::Probe("trim_probe_typing"), |d| {
        d.get_text("content").insert(5, "typed")?;
        Ok(())
    })?;
    let bytes = doc.export_compact_snapshot()?;

    let reloaded = LoroDocument::new("trim-probe-reloaded".to_string())?;
    reloaded.apply_update(&bytes)?;
    assert_eq!(reloaded.get_text("content")?, "seed typed");

    // ALLOW(loro_doc_escape): the UndoManager is a long-lived observer that
    // registers on the doc, the same shape as a subscription.
    let raw = reloaded.doc();
    let undo = UndoManager::new(&raw);
    assert!(
        !undo.can_undo(),
        "a manager built after a reload claims undoable history it cannot own"
    );
    Ok(())
}
