//! BugFunnel `2026-08-31-marks-written-against-stale-content-quarantines-file`
//! (RC-3): one ingest batch that both LENGTHENS a block's content and carries
//! marks positioned in the added tail.
//!
//! `BlockCellRegistry::write_field("marks")` re-asserts the block's STORED text
//! (`update_block_marked`), so the mark spans only address the right string
//! once the same batch's `content` has landed. An ingest batch is a
//! `StorageEntity` (`HashMap`), so pre-fix the two fields were decomposed into
//! field writes in per-call RANDOM order; on the losing half a span from the
//! new parse indexed past the OLD text, Loro rejected the range,
//! `apply_ingest_batch` returned `Err`, and
//! `FileSyncController::on_file_changed` QUARANTINED the file from write-back —
//! Martin's vault hit this four times in 18 hours.
//!
//! This drives `apply_ingest_batch` directly, with the params bag the org
//! reconciler builds (`holon_orgmode::build_block_params`), so the `Err` is
//! observed rather than swallowed by the controller's poll retry. Each round is
//! an independent coin flip pre-fix; forty of them make a false green
//! impossible in practice.
//!
//! The same ordering defect had a SILENT variant in the other direction: when
//! the batch SHORTENS the content, a marks-first write lands the new spans over
//! the old, longer text, where they are in bounds — no `Err`, no quarantine.
//! The following content write then shifts those Peritext anchors, so the block
//! keeps marks at the WRONG offsets and every later reader believes them. The
//! second rung below covers that direction; the write-order rule fixes both.
//!
//! @pbt kind harness
//! @pbt covers marks-lengthening-batch — an ingest batch that lengthens content
//! and marks the appended tail applies whole, so the file is never quarantined
//! @pbt covers marks-shortening-batch — an ingest batch that shortens content
//! stores the mark spans it declared, not silently shifted ones

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::block::Block;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use holon_integration_tests::TestEnvironmentBuilder;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const ROUNDS: usize = 40;
/// Fewer than `ROUNDS`: the shortening rung settles the projection after EVERY
/// revision, so each one costs a quiescence wait. Twenty still leaves a false
/// green at 2^-20.
const SHORTENING_ROUNDS: usize = 20;
const MARKED_WORD: &str = "emphasis";

fn uri(tag: &str) -> EntityUri {
    EntityUri::block(&format!("33333333-0000-4000-8000-00000000{tag}"))
}

/// The block's body at revision `round`: one more leading word each time, with
/// the emphasised word CLOSING the body — so the span ends exactly at the new
/// length and therefore past the previous revision's. (Any trailing text longer
/// than the per-round growth would keep the stale content long enough to
/// swallow the span and hide the defect.) ASCII throughout, so byte and
/// Unicode-scalar offsets coincide.
fn body(round: usize) -> String {
    format!("{}and {MARKED_WORD}", "tail ".repeat(round))
}

#[test]
fn lengthening_content_with_tail_marks_ingests_whole() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("boot an empty vault");
        let ordering = env
            .injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>()
            .await;

        let document = EntityUri::file("Notes.org");
        let parent = uri("aaaa");
        let target = uri("bbbb");

        let persisted = ordering
            .create_in_tree_batch(&[
                BlockCreateRequest {
                    parent_id: EntityUri::no_parent(),
                    id: parent.clone(),
                    content: BlockContent::Text {
                        raw: "Notes".to_string(),
                    },
                    properties: HashMap::new(),
                    edges: BlockEdges::default(),
                },
                BlockCreateRequest {
                    parent_id: parent.clone(),
                    id: target.clone(),
                    content: BlockContent::Text { raw: body(0) },
                    properties: HashMap::new(),
                    edges: BlockEdges::default(),
                },
            ])
            .await
            .expect("create the probe blocks");
        assert!(
            persisted.iter().all(|p| *p),
            "the authority declined a probe create: {persisted:?}"
        );

        for round in 1..=ROUNDS {
            let content = body(round);
            let start = content
                .find(MARKED_WORD)
                .expect("the emphasised word is in the body");

            let mut block = Block::default();
            block.id = target.clone();
            block.parent_id = parent.clone();
            block.content = content.clone();
            block.marks = Some(vec![MarkSpan::new(
                start,
                start + MARKED_WORD.len(),
                InlineMark::Bold,
            )]);

            let params = holon_orgmode::build_block_params(&block, &parent, &document, None);
            ordering
                .apply_ingest_batch(vec![("update".to_string(), params)])
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "round {round}: the ingest batch failed partway, so \
                         `FileSyncController` would QUARANTINE this file from write-back and \
                         render its truncated DB state nowhere. The batch's `marks` write was \
                         applied against the PREVIOUS revision's shorter content, putting the \
                         span ({start}..{}) past the stored text. error: {e:#}",
                        start + MARKED_WORD.len()
                    )
                });
        }

        env.wait_for_loro_quiescence(Duration::from_secs(30)).await;

        let rows = env
            .query_sql(&format!(
                "SELECT content, marks FROM block_raw WHERE id = '{target}'"
            ))
            .await
            .expect("query block_raw");
        let row = rows
            .first()
            .expect("the ingested block has a projected row");
        assert_eq!(
            row.get("content").and_then(|v| v.as_string()),
            Some(body(ROUNDS).as_str()),
            "the last revision's content must be what the store holds"
        );
        assert!(
            !matches!(row.get("marks"), None | Some(holon_api::Value::Null)),
            "the emphasised tail word lost its mark, so a write-back would re-emit it as plain \
             text: {row:?}"
        );
    });
}

/// The silent direction. Shortening the content puts a marks-first write's
/// spans INSIDE the old, longer text, so Loro accepts them and nothing fails;
/// the content write that follows then drags the anchors to the wrong offsets.
///
/// The check runs after EVERY revision rather than once at the end, because a
/// later correctly-ordered revision rewrites the mark set and would paper over
/// an earlier corruption. With the write-order rule bypassed each revision is
/// an independent coin flip, so a run goes red with probability 1 - 2^-20; with
/// the rule in place it is deterministically green.
///
/// Marks are read back off the SQL projection rather than the Loro backend:
/// `TestEnvironment::loro_backend()` is `None` under the default Turso wiring,
/// and the projection is what every downstream reader (and the org write-back)
/// actually consults.
#[test]
fn shortening_content_with_tail_marks_keeps_exact_spans() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("boot an empty vault");
        let ordering = env
            .injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>()
            .await;
        let document = EntityUri::file("Shrinking.org");
        let parent = uri("cccc");
        let target = uri("dddd");

        let persisted = ordering
            .create_in_tree_batch(&[
                BlockCreateRequest {
                    parent_id: EntityUri::no_parent(),
                    id: parent.clone(),
                    content: BlockContent::Text {
                        raw: "Shrinking".to_string(),
                    },
                    properties: HashMap::new(),
                    edges: BlockEdges::default(),
                },
                BlockCreateRequest {
                    parent_id: parent.clone(),
                    id: target.clone(),
                    content: BlockContent::Text {
                        raw: body(SHORTENING_ROUNDS),
                    },
                    properties: HashMap::new(),
                    edges: BlockEdges::default(),
                },
            ])
            .await
            .expect("create the probe blocks");
        assert!(
            persisted.iter().all(|p| *p),
            "the authority declined a probe create: {persisted:?}"
        );

        for round in (0..SHORTENING_ROUNDS).rev() {
            let content = body(round);
            let start = content
                .find(MARKED_WORD)
                .expect("the emphasised word is in the body");
            let declared = vec![MarkSpan::new(
                start,
                start + MARKED_WORD.len(),
                InlineMark::Bold,
            )];

            let mut block = Block::default();
            block.id = target.clone();
            block.parent_id = parent.clone();
            block.content = content.clone();
            block.marks = Some(declared.clone());

            let params = holon_orgmode::build_block_params(&block, &parent, &document, None);
            ordering
                .apply_ingest_batch(vec![("update".to_string(), params)])
                .await
                .unwrap_or_else(|e| panic!("round {round}: shortening ingest batch failed: {e:#}"));

            env.wait_for_loro_quiescence(Duration::from_secs(30)).await;
            let rows = env
                .query_sql(&format!(
                    "SELECT content, marks FROM block_raw WHERE id = '{target}'"
                ))
                .await
                .expect("query block_raw");
            let row = rows
                .first()
                .unwrap_or_else(|| panic!("round {round}: the block has no projected row"));
            // `marks` is a jsonb column, so CDC hands it over as `Value::Json`
            // (never `Value::String`) — the same two shapes `Block::try_from`
            // accepts.
            let marks_json = match row.get("marks") {
                Some(holon_api::Value::Json(s)) | Some(holon_api::Value::String(s)) => s.clone(),
                other => panic!("round {round}: `marks` is {other:?}, expected JSON: {row:?}"),
            };
            let stored_marks = holon_api::marks_from_json(&marks_json)
                .unwrap_or_else(|e| panic!("round {round}: stored marks are not valid JSON: {e}"));

            assert_eq!(
                row.get("content").and_then(|v| v.as_string()),
                Some(content.as_str()),
                "round {round}: the shortened content is not what the store holds"
            );
            assert_eq!(
                stored_marks.as_slice(),
                declared.as_slice(),
                "round {round}: the block kept marks at offsets it was never given. The batch's \
                 `marks` write landed on the PREVIOUS revision's longer text — in bounds, so \
                 nothing failed — and the content write then shifted the anchors. Content is \
                 {content:?} ({} chars).",
                content.chars().count()
            );
        }
    });
}
