//! Transition: bulk external add of blocks via org file write (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:833-860` (generator),
//! `state_machine.rs:3187-3189` (precondition),
//! `state_machine.rs:2457-2480` (ref-state apply),
//! `sut.rs:1565-1847` (SUT apply), and
//! `transition_budgets.rs:233-251` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::block::Block;

use crate::assign_reference_sequences_canonical;
use crate::pbt::sut::E2ESut;
use crate::pbt::types::{apply_org_headline_tag_split, normalize_content_for_org_roundtrip};
use crate::{serialize_blocks_to_org_with_doc, wait_for_file_condition};

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Add multiple blocks to a document by writing an updated org file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BulkExternalAdd {
    pub doc_uri: EntityUri,
    pub blocks: Vec<Block>,
}

impl TransitionFactory<ReferenceState> for BulkExternalAdd {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let doc_uris: Vec<EntityUri> = state.files.documents.keys().cloned().collect();
        check(!doc_uris.is_empty(), Reason::NoDocumentsAvailable).map(|_| {
            // Boost weight when most documents have no editable Text content.
            // Without seeded content, every edit-path generator that filters on
            // `main_editable_descendants` (SplitBlock, Indent, EditViaViewModel,
            // ClickBlock-as-Main, …) returns `None` and the PBT never reaches
            // editing code. Once docs hold Text blocks the weight drops back to
            // base so the rest of the strategy can run.
            let is_empty_doc = |doc_uri: &EntityUri| -> bool {
                !state.domain.block_state.blocks.values().any(|b| {
                    b.parent_id == *doc_uri
                        && b.content_type == ContentType::Text
                        && !b.is_page()
                        && !state.domain.layout_blocks.contains(&b.id)
                })
            };
            let empty_doc_uris: Vec<EntityUri> = doc_uris
                .iter()
                .filter(|u| is_empty_doc(u))
                .cloned()
                .collect();
            let total_docs = doc_uris.len();
            let weight: u32 = if empty_doc_uris.len() * 2 >= total_docs {
                100
            } else {
                1
            };

            // Prefer empty docs when any exist: the multi-block-into-empty-parent
            // path is the cleanest exerciser of the Full-mode CDC ordering invariant
            // (org-parser-assigned sort_keys must agree with the Loro tree's
            // fractional indices). Sampling from the empty subset deterministically
            // hits that path on every BulkExternalAdd, instead of leaving it to a
            // random `select(doc_uris)` to land there.
            let candidate_docs = if !empty_doc_uris.is_empty() {
                empty_doc_uris
            } else {
                doc_uris
            };

            let next_id = state.domain.block_state.next_id;
            let strat = (
                prop::sample::select(candidate_docs),
                prop::collection::vec(
                    (
                        crate::pbt::generators::bulk_content_strategy(),
                        // Parent selector (extended-gen axis 2): block `i`
                        // parents to `sel % (i + 1)` — 0 = the doc, k = bulk
                        // block k-1. Well-founded (earlier blocks only).
                        proptest::num::u8::ANY,
                    ),
                    3..=10,
                ),
            )
                .prop_map(move |(doc_entity_uri, contents)| {
                    let blocks: Vec<Block> = contents
                        .into_iter()
                        .enumerate()
                        .map(|(i, (content, parent_sel))| {
                            let parent = match parent_sel as usize % (i + 1) {
                                k if k > 0 => {
                                    EntityUri::block(&format!("bulk-{}-{}", next_id, k - 1))
                                }
                                _ => doc_entity_uri.clone(),
                            };
                            Block::new_text(
                                EntityUri::block(&format!("bulk-{}-{}", next_id, i)),
                                parent,
                                content,
                            )
                        })
                        .collect();
                    BulkExternalAdd {
                        doc_uri: doc_entity_uri,
                        blocks,
                    }
                })
                .boxed();
            (weight, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for BulkExternalAdd {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(
                state.files.documents.contains_key(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Add all blocks to the reference state, normalizing each
        // block's content the same way `Mutation::apply_to` does for
        // Create. The org renderer round-trips through the parser
        // (which `.trim()`s headlines and `.trim_end()`s content),
        // so the ref must mirror that normalization or `text(col(...))`
        // displays diverge by the trailing-whitespace the parser
        // strips. Without this, `inv-displayed-text` panics on bulk
        // blocks whose generator-produced content ends in a space.
        for block in &self.blocks {
            let mut block = block.clone();
            block.content = normalize_content_for_org_roundtrip(&block.content, block.content_type);
            // A trailing `:tag:` group on the title line re-parses as org TAGS.
            apply_org_headline_tag_split(&mut block);
            let id = block.id.clone();
            state.domain.block_state.blocks.insert(id.clone(), block);
            // Register doc ownership so WriteOrgFile::apply_to_ref's delete
            // cascade can find these on a subsequent file rewrite.
            state
                .domain
                .block_state
                .block_documents
                .insert(id, self.doc_uri.clone());
        }
        // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
        let mut all_blocks: Vec<Block> =
            state.domain.block_state.blocks.values().cloned().collect();
        assign_reference_sequences_canonical(&mut all_blocks);
        state.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();
        state.domain.block_state.next_id += self.blocks.len();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for BulkExternalAdd {
    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut S) {
        sut.apply_bulk_external_add(&self.doc_uri, &self.blocks, ref_state)
            .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for BulkExternalAdd {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let n = self.blocks.len();
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
            writes: n + 2,
            ddl: watches,
            // Each new block triggers org sync CDC; reactive base fires per CDC cycle.
            tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
        }
    }
}

/// SUT-side body of `BulkExternalAdd`. Writes the document's full block list
/// to its org file (canonical order, with document header so custom keywords
/// round-trip), then spawns N concurrent `query_and_watch` tasks against the
/// engine to deliberately race the IVM matview creation — that race is the
/// Flutter startup bug repro this transition exists to gate (`mark_available`
/// scheduler regression + "Database schema changed"/"database is locked"
/// surface). Finally waits for the org file to reflect the expected block
/// count, for the SQL backend to ingest every block, and for the org-files
/// quiescence window to close.
///
/// Extracted from `E2ESut`'s `SutHandle::apply_bulk_external_add` so the trait
/// impl in `sut_handle.rs` stops carrying ~290 lines of transition logic; the
/// impl now just forwards here.
pub async fn apply_bulk_external_add_to_sut(
    sut: &mut E2ESut,
    doc_uri: &EntityUri,
    blocks: &[Block],
    ref_state: &ReferenceState,
) {
    tracing::trace!(
        "[apply] BulkExternalAdd: adding {} blocks to {}",
        blocks.len(),
        doc_uri
    );

    // Resolve file-based URI to UUID-based URI (documents map uses UUID keys after StartApp)
    let resolved_uri = sut.resolve_uri(doc_uri);
    let file_path = sut.ctx.documents.get(&resolved_uri).unwrap_or_else(|| {
        panic!(
            "Document not found for BulkExternalAdd: {} (resolved: {})",
            doc_uri, resolved_uri
        )
    });

    // Get all blocks for this document from reference state.
    // Note: ref_state already includes the new blocks (from apply_reference).
    // Resolve parent_ids so blocks_by_document matches UUID-based doc URIs.
    let resolved_blocks = sut.resolve_ref_blocks(ref_state, true);
    let grouped = holon_api::blocks_by_document(&resolved_blocks);
    let all_blocks: Vec<Block> = grouped
        .into_iter()
        .find(|(uri, _)| *uri == resolved_uri)
        .map(|(_, blocks)| blocks)
        .unwrap_or_default();
    let existing_count = all_blocks.len().saturating_sub(blocks.len());

    // Find the document block for this document (needed for #+TODO: header)
    let doc_block = resolved_blocks
        .iter()
        .find(|b| b.id == resolved_uri && b.is_page());

    // Serialize to org file (with document header so custom keywords round-trip)
    let live_blocks: Vec<&Block> = all_blocks.iter().collect();
    let org_content = serialize_blocks_to_org_with_doc(&live_blocks, &resolved_uri, doc_block);

    tracing::trace!(
        "[BulkExternalAdd] Writing {} total blocks ({} new) to {:?}",
        all_blocks.len(),
        blocks.len(),
        file_path
    );
    // DEBUG: print blocks being serialized
    for b in &all_blocks {
        tracing::trace!(
            "[BulkExternalAdd] block: {} parent_id={} type={}",
            b.id,
            b.parent_id,
            b.content_type
        );
    }
    tracing::trace!("[BulkExternalAdd] ORG CONTENT:\n{}", org_content);
    holon_filesystem::FileSystem::write(
        sut.org_fs.clone().as_ref(),
        file_path,
        org_content.as_bytes(),
    )
    .await
    .expect("Failed to write bulk external add");

    // =========================================================================
    // FLUTTER STARTUP BUG REPRODUCTION:
    // Immediately after writing bulk data, spawn concurrent query_and_watch calls
    // while IVM is still processing the block_with_path materialized view.
    // This simulates what Flutter does: UI requests reactive queries while
    // the backend is still processing the initial data sync.
    // =========================================================================
    // Turso-only: this concurrent-watch race reproduction drives the IVM engine
    // (`query_and_watch` over `block_raw` materialized views). No-Turso has no
    // engine or matviews, so skip straight to the file-content check below; the
    // org→Loro ingest converges via the pre-invariant settle barrier.
    if matches!(sut.ctx.storage(), holon::di::StorageSelector::Turso) {
        let engine = sut.test_ctx().engine();
        let num_concurrent_watches = 3; // Simulate multiple UI components requesting data
        let mut watch_tasks = Vec::new();

        // Timeout for query_and_watch calls.
        // If the OperationScheduler's mark_available bug is present, these calls
        // will hang forever because:
        // 1. query_and_watch creates a materialized view via execute_ddl_with_deps
        // 2. The DDL requires Schema("block") dependency
        // 3. OperationScheduler checks if "block" is in available set - it's NOT
        // 4. Operation is queued in pending, response_rx.await hangs forever
        // 5. mark_available() was never called for core tables during DI init
        let query_timeout = Duration::from_secs(10);

        for i in 0..num_concurrent_watches {
            let engine_clone = engine.clone();
            let prql = format!(
                "from block_raw | select {{id, content}} | filter id != \"bulk-race-{}\" ",
                i
            );
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("PRQL compilation should succeed");
            let task = tokio::spawn(async move {
                let start = Instant::now();
                // Use timeout to detect scheduler hangs
                let result = tokio::time::timeout(
                    query_timeout,
                    engine_clone.query_and_watch(sql.clone(), HashMap::new(), None),
                )
                .await;
                (i, start.elapsed(), sql, result)
            });
            watch_tasks.push(task);
        }

        // Note: Schema initialization happens during app startup via SchemaRegistry.
        // We don't need to test concurrent schema init here - the query_and_watch
        // calls above already test the critical concurrency path.

        // Check results - database lock/schema change errors indicate the Flutter bug
        // These manifest as various error messages:
        // - "database is locked" - SQLite busy timeout expired
        // - "Database schema changed" - IVM detected concurrent schema modifications
        // - "Failed to lock connection pool" - Connection pool contention
        fn is_concurrency_error(error_str: &str) -> bool {
            error_str.contains("database is locked")
                || error_str.contains("Database schema changed")
                || error_str.contains("Failed to lock connection pool")
        }

        for task in watch_tasks {
            match task.await {
                Ok((i, elapsed, _prql, Ok(Ok(_)))) => {
                    tracing::trace!(
                        "[BulkExternalAdd] Concurrent query_and_watch {} succeeded in {:?}",
                        i,
                        elapsed
                    );
                }
                Ok((i, elapsed, prql, Ok(Err(e)))) => {
                    let error_str = format!("{:?}", e);
                    if is_concurrency_error(&error_str) {
                        panic!(
                            "FLUTTER STARTUP BUG REPRODUCED: query_and_watch {} failed with concurrency error \
                             after {:?} while bulk data ({} blocks) was being synced!\n\
                             This is the exact bug that causes Flutter app to get stuck during startup.\n\
                             Query: {}\n\
                             Error: {}",
                            i,
                            elapsed,
                            blocks.len(),
                            prql,
                            error_str
                        );
                    } else {
                        panic!(
                            "Concurrent query_and_watch {} failed after {:?}: {}\nQuery: {}",
                            i, elapsed, error_str, prql
                        );
                    }
                }
                Ok((i, elapsed, prql, Err(_timeout))) => {
                    // Timeout occurred - this indicates the scheduler bug
                    panic!(
                        "SCHEDULER BUG: query_and_watch {} timed out after {:?}!\n\n\
                         Root cause: OperationScheduler's mark_available() was never called for 'blocks' table.\n\n\
                         The materialized view creation is stuck in the scheduler's pending queue:\n\
                         - execute_ddl_with_deps submitted with requires=[Schema(\"blocks\")]\n\
                         - can_execute() returned false (blocks not in available set)\n\
                         - Operation queued in pending, response_rx.await blocks forever\n\n\
                         Query: {}\n\n\
                         Fix required:\n\
                         1. Call scheduler_handle.mark_available() for core tables after schema creation in DI\n\
                         2. Ensure MarkAvailable command calls process_pending_queue() to wake pending ops",
                        i, elapsed, prql
                    );
                }
                Err(e) => {
                    panic!("Query task panicked: {:?}", e);
                }
            }
        }
    }

    // Poll until file contains expected block count (with timeout)
    let expected_block_count = all_blocks.len();
    let file_path_clone = file_path.clone();
    let start = Instant::now();
    let timeout = Duration::from_millis(5000);

    let condition_met = wait_for_file_condition(
        sut.org_fs.as_ref(),
        &file_path_clone,
        |content| {
            let text_count = content.matches(":ID:").count();
            let src_count = content.to_lowercase().matches("#+begin_src").count();
            text_count + src_count == expected_block_count
        },
        timeout,
    )
    .await;

    let elapsed = start.elapsed();
    let final_content =
        holon_filesystem::FileSystem::read_to_string(sut.org_fs.as_ref(), file_path)
            .await
            .expect("Failed to read file after bulk add");
    let text_block_count = final_content.matches(":ID:").count();
    let source_block_count = final_content.to_lowercase().matches("#+begin_src").count();
    let actual_block_count = text_block_count + source_block_count;

    if !condition_met || actual_block_count < expected_block_count {
        panic!(
            "SYNC LOOP BUG: BulkExternalAdd wrote {} blocks but only {} remain after {:?}!\n\
                 Expected {} blocks total ({} existing + {} new).\n\
                 File content:\n{}",
            expected_block_count,
            actual_block_count,
            elapsed,
            expected_block_count,
            existing_count,
            blocks.len(),
            final_content
        );
    }
    tracing::trace!(
        "[BulkExternalAdd] File verified with {} blocks after {:?}",
        actual_block_count,
        elapsed
    );

    // Turso-only: wait for blocks to reach the SQL DB and assert the count
    // (reads `block_raw` via the engine). No-Turso ingests into Loro via the
    // controller's poll loop; convergence is covered by the pre-invariant settle
    // barrier + the org-file-stability poll below, and the block invariants
    // verify the resulting Loro state.
    if matches!(sut.ctx.storage(), holon::di::StorageSelector::Turso) {
        // Now wait for the blocks to sync to the DATABASE.
        let expected_db_count = E2ESut::expected_content_block_count(ref_state);
        let expected_ids = sut.expected_block_ids(ref_state);
        let db_timeout = Duration::from_millis(10000);
        let db_start = Instant::now();

        let actual_rows = sut.wait_for_blocks_synced(&expected_ids, db_timeout).await;
        let db_elapsed = db_start.elapsed();

        if actual_rows.len() == expected_db_count {
            tracing::trace!(
                "[BulkExternalAdd] Database synced ({} blocks) in {:?}",
                expected_db_count,
                db_elapsed
            );
        } else {
            // Diagnostic: print which ref_state blocks are missing from SQL.
            let sql_ids: std::collections::HashSet<String> = actual_rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            let ref_non_doc: Vec<&Block> = ref_state
                .domain
                .block_state
                .blocks
                .values()
                .filter(|b| !b.is_page())
                .collect();
            let mut missing: Vec<String> = Vec::new();
            let mut extra: Vec<String> = Vec::new();
            for b in &ref_non_doc {
                let resolved = sut.resolve_uri(&b.id);
                if !sql_ids.contains(resolved.as_str()) {
                    missing.push(format!(
                        "{} (resolved={}) parent={} doc={:?}",
                        b.id,
                        resolved,
                        b.parent_id,
                        ref_state.domain.block_state.block_documents.get(&b.id)
                    ));
                }
            }
            let ref_ids: std::collections::HashSet<String> = ref_non_doc
                .iter()
                .map(|b| sut.resolve_uri(&b.id).to_string())
                .collect();
            for sid in &sql_ids {
                if !ref_ids.contains(sid) {
                    extra.push(sid.clone());
                }
            }
            panic!(
                "[BulkExternalAdd] WARNING: Database has {} blocks, expected {} after {:?}\n\
             MISSING from SQL ({}):\n  {}\n\
             EXTRA in SQL ({}):\n  {}",
                actual_rows.len(),
                expected_db_count,
                db_elapsed,
                missing.len(),
                missing.join("\n  "),
                extra.len(),
                extra.join("\n  "),
            );
        }
    }

    // Poll until org files stabilize (sync controller finishes re-rendering)
    sut.wait_for_org_files_stable(25, Duration::from_millis(5000))
        .await;
}
