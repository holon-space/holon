//! A warm boot loads the store's documents from their snapshots. Every inline
//! mark must write, read back and expand on such a document exactly as on the
//! fresh document a cold boot makes.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::MarkSpan;
use holon_api::repository::CoreOperations;
use holon_block_roundtrip_testing::inline_mark_strategy;
use holon_loro::LoroDocumentStore;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document_store::DocScope;
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct MarkedBlock {
    text: String,
    marks: Vec<MarkSpan>,
    /// Where one character is typed after the write; a mark's end is the
    /// position its expand policy decides on.
    typed_at: usize,
}

/// One span per mark key: Loro merges two spans of the same key, so a second
/// one would make the round-trip oracle compare against a union.
fn marked_block() -> impl Strategy<Value = MarkedBlock> {
    "[a-z]{1,8}( [a-z]{1,8}){0,3}".prop_flat_map(|text| {
        let len = text.chars().count();
        let span = (inline_mark_strategy(), 0..len, 1..=len).prop_map(|(mark, a, b)| {
            let (start, end) = if a < b { (a, b) } else { (b - 1, a + 1) };
            MarkSpan::new(start, end, mark)
        });
        (prop::collection::vec(span, 1..5), 0..=len).prop_map(move |(spans, typed_at)| {
            let mut keys = HashSet::new();
            let marks = spans
                .into_iter()
                .filter(|s| keys.insert(s.mark.loro_key()))
                .collect();
            MarkedBlock {
                text: text.clone(),
                marks,
                typed_at,
            }
        })
    })
}

fn sorted(mut marks: Vec<MarkSpan>) -> Vec<MarkSpan> {
    marks.sort_by_key(|s| (s.start, s.end, s.mark.loro_key()));
    marks
}

#[derive(Debug, PartialEq)]
struct Written {
    after_write: Vec<MarkSpan>,
    after_typing: Vec<MarkSpan>,
}

async fn write_and_type(store: &LoroDocumentStore, block: &MarkedBlock) -> Result<Written, String> {
    let backend = LoroBackend::from_document(
        store
            .get_doc(DocScope::Global)
            .await
            .map_err(|e| format!("{e:#}"))?,
        std::sync::Arc::new(holon_api::SystemClock),
    );
    let id = EntityUri::block("marked");
    backend
        .create_block_with_properties(
            EntityUri::no_parent(),
            BlockContent::RichText {
                text: block.text.clone(),
                marks: block.marks.clone(),
            },
            Some(id.clone()),
            &HashMap::new(),
            &BlockEdges::default(),
        )
        .await
        .map_err(|e| format!("write: {e}"))?;
    let marks_of = |b: holon_api::block::Block| sorted(b.marks.unwrap_or_default());
    let after_write = marks_of(
        backend
            .get_block(id.as_str())
            .await
            .map_err(|e| e.to_string())?,
    );
    backend
        .insert_text(id.as_str(), block.typed_at, "x")
        .await
        .map_err(|e| format!("typing: {e}"))?;
    let after_typing = marks_of(
        backend
            .get_block(id.as_str())
            .await
            .map_err(|e| e.to_string())?,
    );
    Ok(Written {
        after_write,
        after_typing,
    })
}

/// The cold-boot store writes into its fresh document; the warm-boot store
/// over the same directory writes into the document loaded from the cold
/// store's snapshot.
fn cold_and_warm(block: &MarkedBlock) -> (Result<Written, String>, Result<Written, String>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let cold_dir = tempfile::tempdir().unwrap();
        let cold = LoroDocumentStore::new(cold_dir.path().to_path_buf()).with_peer_id(Some(1));
        let cold_written = write_and_type(&cold, block).await;

        let warm_dir = tempfile::tempdir().unwrap();
        let first_boot =
            LoroDocumentStore::new(warm_dir.path().to_path_buf()).with_peer_id(Some(1));
        first_boot.get_doc(DocScope::Global).await.unwrap();
        first_boot.save_all().await.unwrap();
        let warm = LoroDocumentStore::new(warm_dir.path().to_path_buf()).with_peer_id(Some(1));
        (cold_written, write_and_type(&warm, block).await)
    })
}

fn check(block: &MarkedBlock) -> Result<(), TestCaseError> {
    let (cold, warm) = cold_and_warm(block);
    let cold = cold.map_err(|e| TestCaseError::fail(format!("cold boot: {e}")))?;
    prop_assert_eq!(
        &cold.after_write,
        &sorted(block.marks.clone()),
        "the marks must round-trip on a cold boot"
    );
    let warm = warm.map_err(|e| TestCaseError::fail(format!("warm boot: {e}")))?;
    prop_assert_eq!(
        &warm,
        &cold,
        "a warm boot must store and expand marks as a cold boot does"
    );
    Ok(())
}

/// An org headline with a verbatim span: the mark whose write failed with
/// `StyleConfigMissing` when a template was ingested after a warm boot.
fn verbatim_headline() -> MarkedBlock {
    let (text, marks) = holon_org_format::extract_block_marks("run =cargo test= now");
    assert!(
        marks.iter().any(|s| s.mark == holon_api::InlineMark::Verbatim),
        "the headline parses to a verbatim mark: {marks:?}"
    );
    let typed_at = text.chars().count();
    MarkedBlock {
        text,
        marks,
        typed_at,
    }
}

#[test]
fn every_mark_behaves_alike_on_a_warm_and_a_cold_boot() {
    check(&verbatim_headline()).unwrap();
    proptest::test_runner::TestRunner::new(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(&marked_block(), |block| check(&block))
    .unwrap();
}
