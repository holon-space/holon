//! Measurement: `LoroDocument::with_write` isolates a batch; it does NOT roll
//! one back.
//!
//! The distinction decides whether `with_write` can serve as the atomicity
//! seam for a multi-op batch — the question the `dense_patch` all-or-nothing
//! work ran into (bugfunnel `2026-09-17-dense-patch-apply-is-not-atomic`).
//! Its doc comment promises only that no reader, exporter or saver observes
//! the batch interior. Read quickly, that sounds like a transaction. It is
//! not, and this file pins the difference by measuring it.
//!
//! VERDICT: a closure that returns `Err` keeps every mutation it already
//! made. The scope does flush them under its OWN origin before its guard
//! drops, so they are attributed to the batch that made them — but they are
//! not undone. Any future attempt to build all-or-nothing block writes on
//! `with_write` has to defeat this test first.

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use anyhow::bail;
use holon_loro::LoroDocument;
use holon_loro::WriteOrigin;

fn text_of(doc: &LoroDocument) -> Result<String> {
    doc.with_read(|d| Ok(d.get_text("content").to_string()))
}

#[test]
fn a_failed_write_batch_keeps_its_earlier_mutations() -> Result<()> {
    let doc = LoroDocument::new("with_write_abort".to_string())?;

    let outcome = doc.with_write(WriteOrigin::Probe("failing_batch"), |d| {
        d.get_text("content").insert(0, "STEP-ONE")?;
        bail!("step two fails, after step one already mutated the doc");
        #[allow(unreachable_code)]
        Ok(())
    });
    assert!(outcome.is_err(), "the closure must propagate its failure");

    assert_eq!(
        text_of(&doc)?,
        "STEP-ONE",
        "with_write does not discard a failed batch's earlier mutations"
    );

    Ok(())
}

/// The half that used to make the first one dangerous: the failed batch's ops
/// once rode out on the next unrelated commit, carrying THAT batch's origin
/// rather than their own. They are now flushed by the scope that made them, so
/// each op reaches subscribers under the origin of its own writer.
#[test]
fn a_failed_batch_commits_its_own_ops_under_its_own_origin() -> Result<()> {
    let doc = LoroDocument::new("with_write_leak".to_string())?;

    let origins: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = origins.clone();
    // ALLOW(loro_doc_escape): subscription registration, a blessed use.
    let _sub = doc.doc().subscribe_root(Arc::new(move |event| {
        seen.lock().unwrap().push(event.origin.to_string());
    }));

    let outcome = doc.with_write(WriteOrigin::Probe("failing_batch"), |d| {
        d.get_text("content").insert(0, "STEP-ONE")?;
        bail!("step two fails");
        #[allow(unreachable_code)]
        Ok(())
    });
    assert!(outcome.is_err());

    doc.with_write(WriteOrigin::Probe("later_unrelated_write"), |d| {
        d.get_text("content").insert(0, "LATER|")?;
        Ok(())
    })?;

    assert_eq!(
        text_of(&doc)?,
        "LATER|STEP-ONE",
        "the failed batch's op is still in the doc — this is isolation, not rollback"
    );
    assert_eq!(
        *origins.lock().unwrap(),
        vec![
            WriteOrigin::Probe("failing_batch").as_origin(),
            WriteOrigin::Probe("later_unrelated_write").as_origin(),
        ],
        "each op must reach subscribers under the origin of the batch that made it"
    );

    Ok(())
}
