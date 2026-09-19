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
//! VERDICT: a closure that returns `Err` keeps every mutation it already made,
//! and the NEXT successful batch commits them under ITS origin. So a failed
//! batch is not undone — it is deferred and re-labelled, which is strictly
//! worse for a caller than a failure it can see. Any future attempt to build
//! all-or-nothing block writes on `with_write` has to defeat this test first.

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

/// The half that makes the first one dangerous: the pending ops a failed batch
/// left behind ride out on the next unrelated commit, carrying THAT batch's
/// origin rather than their own.
#[test]
fn a_later_batch_commits_what_the_failed_one_left_pending() -> Result<()> {
    let doc = LoroDocument::new("with_write_leak".to_string())?;

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
        "the failed batch's op is still in the doc and rode out on the next commit"
    );

    Ok(())
}
