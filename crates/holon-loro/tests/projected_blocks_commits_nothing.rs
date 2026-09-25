//! `LoroBackend::projected_blocks` reads a copy of the doc. Taking that copy
//! must not commit ops some writer left pending, because a commit is what
//! labels ops with their writer's origin.

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use holon_loro::LoroDocument;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::LoroBackend;

fn recorded_origins(doc: &LoroDocument) -> (Arc<Mutex<Vec<String>>>, loro::Subscription) {
    let origins: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = origins.clone();
    // ALLOW(loro_doc_escape): subscription registration; the callback never reads
    // the doc.
    let sub = doc.doc().subscribe_root(Arc::new(move |event| {
        seen.lock().unwrap().push(event.origin.to_string());
    }));
    (origins, sub)
}

#[test]
fn projected_blocks_inside_a_write_batch_is_refused_and_the_batch_stays_one_commit() -> Result<()> {
    let doc = Arc::new(LoroDocument::new("projected_blocks_in_batch".to_string())?);
    let backend = LoroBackend::from_document(doc.clone());
    let (origins, _sub) = recorded_origins(&doc);

    let inner = doc.with_write(WriteOrigin::Probe("batch"), |d| {
        d.get_text("content").insert(0, "ONE|")?;
        let inner = backend.projected_blocks().map(|blocks| blocks.len());
        d.get_text("content").insert(4, "TWO")?;
        Ok(inner)
    })?;

    assert_eq!(
        *origins.lock().unwrap(),
        vec![WriteOrigin::Probe("batch").as_origin().to_string()],
        "the batch must reach subscribers as one commit under its own origin"
    );
    let err = inner.expect_err("a projection read inside a write batch must be refused");
    assert!(err.to_string().contains("write batch"), "{err}");
    Ok(())
}

#[test]
fn projected_blocks_refuses_ops_left_pending_outside_the_write_guard() -> Result<()> {
    let doc = Arc::new(LoroDocument::new("projected_blocks_pending".to_string())?);
    let backend = LoroBackend::from_document(doc.clone());
    let (origins, _sub) = recorded_origins(&doc);

    // ALLOW(loro_doc_escape): the unguarded write this test plants on purpose.
    doc.doc().get_text("content").insert(0, "UNGUARDED")?;

    let read = backend.projected_blocks();
    assert!(
        origins.lock().unwrap().is_empty(),
        "the read committed someone else's op: {:?}",
        origins.lock().unwrap()
    );
    let err = read.expect_err("a fork would commit the pending op with no origin");
    assert!(err.to_string().contains("uncommitted"), "{err}");
    Ok(())
}
