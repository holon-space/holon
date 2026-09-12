//! A non-echo text delta must reach the editor on the cell leg.
//!
//! A cell-attached editor converges SOLELY through
//! `Cell::remote_deltas()` — the render backstop is deliberately off for it
//! (`frontends/gpui/src/render/builders/editable_text.rs:130-138`), because the
//! cell is meant to be the single external content source. So a delta that the
//! cell backing accepts but never publishes leaves the editor showing stale
//! text with nothing to cure it.
//!
//! This is not an undo wrinkle: the same hop carries a peer's merge and an org
//! ingest to a block the user has open.

// The harness serves several binaries; this one drives only its boot.
#[allow(dead_code)]
#[path = "session_shutdown/harness.rs"]
mod harness;

use std::any::TypeId;
use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::cell::Cell;
use holon_frontend::cell::EntityCellRegistry;

const PROBE_CHILD: &str = "shutdown-probe-child";

/// The content cell the EDITOR gets, on the leg the editor runs.
async fn editor_cell(booted: &harness::Booted) -> Cell<String> {
    let registry = booted
        .injector
        .optional_resolve_async::<holon_loro::block_cell_registry::BlockCellRegistry>()
        .await
        .expect("a booted CRDT session registers a BlockCellRegistry");
    let uri = EntityUri::block(PROBE_CHILD);
    registry
        .editable_field_any(&uri, "content", TypeId::of::<String>())
        .expect("the probe block's content cell")
        .downcast::<Cell<String>>()
        .map(|c| (*c).clone())
        .expect("content resolves to a Cell<String>")
}

/// Write the block's content through the production dispatcher — the shape an
/// org ingest takes, and a `sys.`-origin write the editor must converge to.
async fn authoritative_write(booted: &harness::Booted, text: &str) {
    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(PROBE_CHILD.to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String(text.to_string()));
    booted
        .engine
        .execute_operation(
            &holon_api::EntityName::from("block"),
            "set_field",
            params,
            OpOrigin::Ingest,
        )
        .await
        .expect("block/set_field through the production dispatcher");
}

/// The delta must WAKE the editor's subscription.
///
/// `remote_deltas()` is the only channel a cell-attached editor has. The
/// assertion is on the wakeup arriving, because that is what the editor waits
/// on; the value it then reads is asserted by the sibling test below.
#[tokio::test(flavor = "multi_thread")]
async fn an_authoritative_write_wakes_the_editors_delta_stream() {
    let booted = harness::boot_a_working_session().await;
    let cell = editor_cell(&booted).await;
    let mut deltas = cell.remote_deltas();

    authoritative_write(&booted, "rewritten by the file on disk").await;

    let woke = tokio::time::timeout(Duration::from_secs(10), deltas.next()).await;
    assert!(
        matches!(woke, Ok(Some(_))),
        "an authoritative write to a block the editor holds never woke its delta stream, so a \
         cell-attached editor keeps painting the old text with no other channel to cure it \
         (the render backstop is off for cell editors by design)"
    );
}

/// And the value the editor would then read must be the new one.
#[tokio::test(flavor = "multi_thread")]
async fn an_authoritative_write_reaches_the_cell_the_editor_reads() {
    let booted = harness::boot_a_working_session().await;
    let cell = editor_cell(&booted).await;
    let before = cell.current();

    authoritative_write(&booted, "rewritten by the file on disk").await;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if cell.current() != before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        cell.current(),
        "rewritten by the file on disk",
        "the cell the editor converges to never took the authoritative write"
    );
}

/// The shape the windowed rung fails on: the delta an UNDO produces.
///
/// Loro stamps its own undo commits with origin `undo`, which the cell
/// backing's echo filter does not suppress, so it should reach the editor
/// exactly like any other authoritative write.
#[tokio::test(flavor = "multi_thread")]
async fn an_undo_delta_wakes_the_editors_delta_stream() {
    let booted = harness::boot_a_working_session().await;
    let store = booted
        .injector
        .try_resolve::<holon_loro::LoroDocumentStore>()
        .expect("a booted CRDT session registers a LoroDocumentStore");
    let cell = editor_cell(&booted).await;
    let undo = store
        .text_undo()
        .expect("resolving an editor cell arms the text-undo manager");

    let before = cell.current();
    cell.apply_text_op(holon_core::cell::TextOp::Insert {
        pos_codepoint: 0,
        text: "typed".to_string(),
    })
    .expect("a keystroke into the content cell");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !undo.can_undo().unwrap() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(undo.can_undo().unwrap(), "the keystroke was never recorded");

    // Subscribe AFTER the typing, exactly as a focused editor already is.
    let mut deltas = cell.remote_deltas();
    assert!(undo.undo().expect("undo"), "the undo took nothing back");

    let woke = tokio::time::timeout(Duration::from_secs(10), deltas.next()).await;
    assert!(
        matches!(woke, Ok(Some(_))),
        "the undo's delta never woke the editor's stream, so a cell-attached editor keeps \
         painting the typed text after cmd-z"
    );
    assert_eq!(
        cell.current(),
        before,
        "the cell the editor converges to did not return to the pre-typing text"
    );
}
