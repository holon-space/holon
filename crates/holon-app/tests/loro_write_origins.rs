//! Every Loro commit a booted session makes names the seam that made it.
//!
//! The text-undo manager of the cell-undo lane takes back exactly one thing:
//! the characters the user typed into an editor cell. It selects them by the
//! commit origin, so a write that is NOT a keystroke — boundary ingest, a
//! dispatcher operation, a peer merge — must carry an origin the manager can
//! exclude. One shared prefix makes that exclusion fail-safe: a seam added
//! later is excluded unless it deliberately claims to be a keystroke.

// The harness serves two other binaries too; this one drives only its boot.
#[allow(dead_code)]
#[path = "session_shutdown/harness.rs"]
mod harness;

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::cell::Cell;
use holon_core::cell::TextOp;
use holon_frontend::cell::EntityCellRegistry;
use holon_loro::loro_document_store::DocScope;
use holon_loro::loro_document_store::LoroDocumentStore;
use holon_loro::write_origin::WriteOrigin;

/// The block the probe vault gives every test here to write into.
const PROBE_CHILD: &str = "shutdown-probe-child";

/// Record the origin of every commit on the session's global document.
///
/// The subscription is returned alongside the sink and must be held: dropping
/// it unsubscribes, and the test would then measure nothing and pass
/// vacuously.
async fn watch_origins(
    injector: &fluxdi::Injector,
) -> (Arc<Mutex<Vec<String>>>, Box<dyn std::any::Any>) {
    let store = injector
        .try_resolve::<LoroDocumentStore>()
        .expect("a booted CRDT session registers a LoroDocumentStore");
    let doc = store
        .get_doc(DocScope::Global)
        .await
        .expect("the global Loro document");

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    // ALLOW(loro_doc_escape): subscription registration, a blessed use.
    let sub = doc.doc().subscribe_root(Arc::new(move |event| {
        sink.lock().unwrap().push(event.origin.to_string());
    }));
    (seen, Box::new(sub))
}

fn drain(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    std::mem::take(&mut *seen.lock().unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_boundary_ingest_write_does_not_look_like_typing() {
    let booted = harness::boot_a_working_session().await;
    let (seen, _sub) = watch_origins(&booted.injector).await;

    // The shape `holon_app::seed` and the org-ingest leg both use: a block
    // operation dispatched with `OpOrigin::Ingest`.
    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(PROBE_CHILD.to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert(
        "value".into(),
        Value::String("rewritten by the file on disk".to_string()),
    );
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

    let origins = drain(&seen);
    assert!(
        !origins.is_empty(),
        "the dispatched ingest wrote nothing to Loro, so this test proves nothing"
    );
    for origin in &origins {
        assert!(
            origin.starts_with(WriteOrigin::SYSTEM_PREFIX),
            "an ingest write committed with origin {origin:?}, which the text-undo manager \
             cannot exclude; every non-keystroke seam must carry the {:?} prefix. Saw: {origins:?}",
            WriteOrigin::SYSTEM_PREFIX
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_through_the_cell_is_the_only_undoable_origin() {
    let booted = harness::boot_a_working_session().await;
    let (seen, _sub) = watch_origins(&booted.injector).await;

    let registry = booted
        .injector
        .optional_resolve_async::<holon_loro::block_cell_registry::BlockCellRegistry>()
        .await
        .expect("a booted CRDT session registers a BlockCellRegistry");
    let uri = EntityUri::block(PROBE_CHILD);
    let cell: Cell<String> = registry
        .editable_field_any(&uri, "content", TypeId::of::<String>())
        .expect("the probe block's content cell")
        .downcast::<Cell<String>>()
        .map(|c| (*c).clone())
        .expect("content resolves to a Cell<String>");

    cell.apply_text_op(TextOp::Insert {
        pos_codepoint: 0,
        text: "x".to_string(),
    })
    .expect("a keystroke into the content cell");

    let origins = drain(&seen);
    // The property is that the keystroke is the ONLY write escaping the system
    // prefix — not that nothing else commits meanwhile. Boot ingest can still
    // be settling, and asserting quiescence would assert something the session
    // never promised.
    let keystroke = WriteOrigin::UiEditorKeystroke.as_origin().to_string();
    assert!(
        origins.contains(&keystroke),
        "the keystroke did not commit under the keystroke origin; saw {origins:?}"
    );
    assert!(
        origins
            .iter()
            .all(|o| *o == keystroke || o.starts_with(WriteOrigin::SYSTEM_PREFIX)),
        "a write other than the keystroke escaped the system prefix: {origins:?}"
    );
    assert!(
        !WriteOrigin::UiEditorKeystroke
            .as_origin()
            .starts_with(WriteOrigin::SYSTEM_PREFIX),
        "the keystroke origin must NOT carry the system prefix, or the undo manager would \
         exclude the one thing it exists to take back"
    );
}
