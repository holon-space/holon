//! `update_block_marked` installs a text and a mark set together, so a span
//! reaching past that text is a caller desync — the marks were measured against
//! different content. Loro's own rejection is an `OutOfBound { pos, len }` that
//! names neither the block nor the string it was measured against, which is how
//! BugFunnel `2026-08-31-marks-written-against-stale-content-quarantines-file`
//! reached Martin as a quarantine mystery. Fail at the write, naming both
//! sides.
//!
//! @pbt kind harness
//! @pbt covers marks-span-precondition — an out-of-range mark span is rejected
//! by name (block, span, text length) before Loro sees it

use std::sync::Arc;

use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::repository::CoreOperations;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document::LoroDocument;

async fn backend_with_block(id: &str) -> Arc<LoroBackend> {
    let doc = LoroDocument::new("marks-precondition".to_string()).expect("loro doc");
    let backend = Arc::new(LoroBackend::from_document(Arc::new(doc)));
    let root = backend.create_placeholder_root("root").await.expect("root");
    backend
        .create_block(
            holon_api::EntityUri::from_raw(&root),
            holon_api::BlockContent::text("short"),
            Some(holon_api::EntityUri::from_raw(id)),
        )
        .await
        .expect("create block");
    backend
}

#[tokio::test]
async fn span_past_the_installed_text_is_rejected_by_name() {
    let id = "block:marked";
    let backend = backend_with_block(id).await;

    // The shape the ingest produced: marks measured against the NEW, longer
    // parse while the text handed over is the OLD, shorter one.
    let err = backend
        .update_block_marked(id, "short", &[MarkSpan::new(2, 40, InlineMark::Bold)])
        .await
        .expect_err("an out-of-range mark span must not reach Loro");
    let message = format!("{err}");

    for needle in [id, "2..40", "5-char"] {
        assert!(
            message.contains(needle),
            "the rejection must name {needle:?} so the desync is diagnosable at the write; got: \
             {message}"
        );
    }
}

#[tokio::test]
async fn span_inside_the_installed_text_still_applies() {
    let id = "block:marked";
    let backend = backend_with_block(id).await;

    // The same span is legal once the text this call installs is the longer one
    // — the precondition gates on the batch's own content, not on the store's.
    backend
        .update_block_marked(
            id,
            "a considerably longer replacement body",
            &[MarkSpan::new(2, 14, InlineMark::Bold)],
        )
        .await
        .expect("an in-range span over the newly installed text must apply");

    let block = backend.get_block(id).await.expect("read back");
    assert_eq!(block.content, "a considerably longer replacement body");
    assert_eq!(
        block.marks.as_deref().unwrap_or_default(),
        &[MarkSpan::new(2, 14, InlineMark::Bold)],
        "the mark set must survive the write it was validated against"
    );
}
