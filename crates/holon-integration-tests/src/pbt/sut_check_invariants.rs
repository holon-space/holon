//! `apply_transition_async` + the per-transition settle scaffolding
//! (the SplitBlock barrier in `block_tree_post_action`, the CDC drains, the
//! `assert_cdc_quiescent` barrier) + the live-data mirror accessors
//! (`live_blocks`, `live_focus_roots`, `wait_for_live_data_mirrors`).
//!
//! Invariant checking itself lives entirely in `run_invariant_registry`
//! (`pbt/invariant_runner.rs`): the registry runner owns the doc-URI
//! resolution, the `block_raw` convergence wait, and every registered body.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

use super::reference_state::ReferenceState;
use super::sut::E2ESut;
use super::sut_cdc_mirrors::FocusRoot;
use super::types::*;

impl E2ESut {
    /// `ref_state`-dependent post-action for block-mutating transitions.
    ///
    /// The cap-trait SUT methods (`SutBlockTreeWrite::apply_split_block`
    /// / `apply_join_block` / indent / outdent / move_*) are pure actions
    /// with no `ref_state` parameter. The sync barrier + block-count check
    /// + synthetic-id reconciliation that used to live inside the old
    /// `SutHandle::apply_split_block`/`apply_join_block` tails are
    /// `ref_state`-dependent, so they move here, where the harness owns
    /// `ref_state`. No behaviour loss: the sync still happens, the count
    /// is still asserted, and `map_unmapped_split_synthetic_ids` still
    /// mutates `doc_uri_map` so later transitions resolve the new block.
    async fn block_tree_post_action(
        &mut self,
        ref_state: &ReferenceState,
        transition: &crate::pbt::transitions::E2ETransition,
    ) {
        use crate::pbt::transitions::E2ETransition;
        match transition {
            E2ETransition::SplitBlock(_) => {
                let expected_count = Self::expected_content_block_count(ref_state);
                let expected_ids = self.expected_block_ids(ref_state);
                let timeout = std::time::Duration::from_secs(5);
                // Turso waits for the CDC accumulator to catch up to `block_raw`;
                // no-Turso's Loro mutation is synchronous, so read the snapshot
                // directly (both return non-page content rows with an `id`).
                let db_rows = if matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
                    self.wait_for_blocks_synced(&expected_ids, timeout).await
                } else {
                    self.ctx.non_page_block_rows().await
                };
                if db_rows.len() != expected_count {
                    let id_vec: Vec<String> = db_rows
                        .iter()
                        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                        .collect();
                    let actual_ids: HashSet<EntityUri> =
                        // ALLOW(entity_uri_from_raw): id fields from db_rows snapshot rows
                        id_vec.iter().map(|s| EntityUri::from_raw(s)).collect();
                    let mut id_counts: std::collections::HashMap<String, u32> = HashMap::new();
                    for id in &id_vec {
                        *id_counts.entry(id.clone()).or_insert(0) += 1;
                    }
                    let duplicates: Vec<(String, u32)> = id_counts
                        .iter()
                        .filter(|(_, c)| **c > 1)
                        .map(|(k, v)| (k.clone(), *v))
                        .collect();
                    let missing: Vec<&EntityUri> = expected_ids.difference(&actual_ids).collect();
                    let extra: Vec<&EntityUri> = actual_ids.difference(&expected_ids).collect();
                    eprintln!(
                        "[SplitBlock count-mismatch diag] expected={} db_rows={} unique_ids={} duplicates={:?} missing_from_block_raw={:?} extra_in_block_raw={:?}",
                        expected_count,
                        db_rows.len(),
                        actual_ids.len(),
                        duplicates,
                        missing,
                        extra,
                    );
                }
                assert_eq!(
                    db_rows.len(),
                    expected_count,
                    "[SplitBlock] Block count mismatch after split"
                );

                // Capture pre-split known real ids so we can identify the freshly
                // created block among `db_rows` (mirrors map_unmapped_split_synthetic_ids).
                let pre_known: HashSet<String> = {
                    let map = self.doc_uri_map.lock().unwrap();
                    let mut ids: HashSet<String> = map.values().map(|u| u.to_string()).collect();
                    for ref_id in ref_state.domain.block_state.blocks.keys() {
                        if !map.contains_key(ref_id) && !crate::pbt::is_synthetic_ref_id(ref_id) {
                            ids.insert(ref_id.to_string());
                        }
                    }
                    ids
                };
                self.map_unmapped_split_synthetic_ids(ref_state, "[SplitBlock]")
                    .await;

                // Park the headless editor mirror's caret at the new block's
                // start. Since ADR 0010, prod's `split_block` returns the new
                // focus `{block_id, cursor_offset}` in its op response and the
                // frontend dispatch hook sets `UiState.focused_block` + the
                // caret seed in-process — so focus itself needs no SUT action.
                // The headless mirror, though, tracks the caret in its own
                // per-block map (it doesn't read the gpui caret seed) and routes
                // every keystroke at `engine.focused_block()`, so without this a
                // following `PressKey(Enter)` / `TypeChars` would hit the wrong
                // caret (the `inv-blocks-match-ref` content divergence).
                // Identify the fresh block BY ELIMINATION — and assert the
                // elimination is unambiguous. The count assert above guarantees
                // total cardinality, but if the SUT minted two blocks and
                // dropped a pre-known one (count matches, wrong survivor) the
                // old `.find()` could adopt the wrong id and then steer the
                // caret toward it, laundering the divergence.
                let unknown_ids: Vec<String> = db_rows
                    .iter()
                    .filter_map(|row| row.get("id")?.as_string().map(|s| s.to_string()))
                    .filter(|id| !pre_known.contains(id))
                    .collect();
                assert!(
                    unknown_ids.len() <= 1,
                    "[SplitBlock] expected exactly one freshly-minted block, found {}: {:?} \
                     — a pre-known block must have vanished (count assert passed)",
                    unknown_ids.len(),
                    unknown_ids
                );
                if let Some(new_id) = unknown_ids.first() {
                    // ALLOW(entity_uri_from_raw): id field from db_rows snapshot row
                    self.sync_caret_to_new_split_block(&EntityUri::from_raw(new_id))
                        .await;
                }
            }
            E2ETransition::JoinBlock(_) => {
                // Turso-only sync barrier; no-Turso's Loro join is synchronous.
                if matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
                    let expected_ids = self.expected_block_ids(ref_state);
                    self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
                        .await;
                }
            }
            _ => {}
        }
    }

    /// Lazy accessor for the CDC-driven `LiveData<Block>` mirroring the `block`
    /// matview. Built on first use because we need an async `watch_view` call and
    /// the SUT struct can't carry a started engine at construction time. The
    /// matview hydrates `tags` (and `requires`) from the junction tables, so
    /// rows are read directly into a fully-populated `Block`.
    pub(super) async fn live_blocks(&self) -> Arc<holon::sync::LiveData<Block>> {
        self.cdc.blocks(self.ctx.engine()).await
    }

    /// Lazy accessor for the CDC-driven `LiveData<FocusRoot>` mirroring the
    /// `focus_roots` matview. Keyed by `"{region}\u{1F}{root_id}"` since one
    /// region can have multiple root rows (one per child of the nav target).
    pub(super) async fn live_focus_roots(&self) -> Arc<holon::sync::LiveData<FocusRoot>> {
        self.cdc.focus_roots(self.ctx.engine()).await
    }

    /// Async body of `apply()` — extracted so Flutter (already async) can call directly
    /// without `block_on`.
    #[tracing::instrument(skip(self, ref_state, transition), name = "pbt.apply_transition")]
    pub async fn apply_transition_async(
        &mut self,
        ref_state: &ReferenceState,
        transition: &crate::pbt::transitions::E2ETransition,
    ) {
        use holon_pbt_core::TransitionImpl;
        transition.apply_to_sut(ref_state, self).await;

        // Block-mutating transitions that mint or delete blocks need a
        // `ref_state`-dependent post-action that the action method itself
        // can't carry (the cap-trait `apply_split_block`/`apply_join_block`
        // are pure actions). SplitBlock additionally reconciles the
        // freshly-minted prod UUID back onto the synthetic `block:split-N`
        // slot the ref-state allocated (`doc_uri_map` bookkeeping) — drop
        // this and later transitions can't resolve the new block. Run it
        // here, where the harness has `ref_state`.
        self.block_tree_post_action(ref_state, transition).await;

        // Stash the post-transition ref-state so the NEXT call can read it
        // as its pre-transition state. The framework hands us only the
        // post-state, so we have to carry the previous post forward
        // ourselves. See `pre_ref_state` field doc for the rationale.
        self.pre_ref_state = Some(ref_state.clone());

        // File-writing transitions (WriteOrgFile / CreateDocument) reach the
        // SQL sink via the OS file-watcher → `on_file_changed` → projection
        // chain. None of the settle barriers below await that FS-event round
        // trip — they cover the EventBus (`org`/`cache`) and Loro paths only.
        // For a post-StartApp `index.org` *swap*, `apply_write_org_file`
        // resolves the already-seeded doc URI instantly and returns before the
        // watcher ingests the new content, so the watermark in
        // `assert_cdc_quiescent` is sampled too early: the legitimate one-time
        // block `Created` lands at `seq > target` and is misreported as churn.
        // Wait for the file's blocks to reach `block_raw` first. (Diagnosed
        // 2026-05-22; the slices' `index.org` filter is the same race.)
        // `wait_for_blocks_synced` reads Turso's CDC accumulator (`block_raw`);
        // it is the Turso path's catch-up after a file write. The no-Turso path
        // ingests org files through the Loro-wired `FileSyncController` and
        // converges via the pre-invariant settle barrier instead.
        if self.ctx.is_running()
            && matches!(transition.variant_name(), "WriteOrgFile" | "CreateDocument")
            && matches!(self.ctx.storage(), holon::di::StorageSelector::Turso)
        {
            let expected_ids = self.expected_block_ids(ref_state);
            // Same-id rewrites (an `index.org` layout swap keeps the fixed
            // `block:root-layout` ids and changes only content) are invisible
            // to the id-presence predicate — also require the written blocks'
            // content to converge. Ref content is org-roundtrip normalized in
            // `WriteOrgFile::apply_to_ref`, so equality is exact.
            let expected_contents: std::collections::HashMap<EntityUri, String> =
                if let crate::pbt::transitions::E2ETransition::WriteOrgFile(w) = transition {
                    w.blocks
                        .iter()
                        .filter_map(|b| {
                            ref_state
                                .domain
                                .block_state
                                .blocks
                                .get(&b.id)
                                .map(|rb| (self.resolve_uri(&b.id), rb.content.clone()))
                        })
                        .collect()
                } else {
                    Default::default()
                };
            self.wait_for_blocks_synced_with_content(
                &expected_ids,
                &expected_contents,
                std::time::Duration::from_secs(5),
            )
            .await;
        }

        // Yield to let tokio schedule CDC forwarding tasks before we drain.
        tokio::task::yield_now().await;
        self.drain_cdc_events().await;
        self.drain_region_cdc_events().await;

        // Drain both directions of the Loro mirror BEFORE sampling
        // `target_seq` in `assert_cdc_quiescent`. The original layout ran
        // `wait_for_consumers` AFTER the inv-editable-text-has-draggable assert, which let SQL writes
        // produced by inbound EventBus consumers (e.g. `LoroSyncController`'s
        // SQL→Loro path triggering an outbound Loro→SQL reconcile) commit
        // *during* the inv-editable-text-has-draggable grace window — looking like spurious churn
        // when they're really just causally-related writes that haven't
        // settled yet.
        //
        // Settle path that has to converge before the assert: a Loro write
        // (org intent / chord op) → `subscribe_root` → `on_loro_changed`
        // projects the SQL rows → CDC → cache/watch consumers. Loro is the
        // authority; there is no SQL→Loro reflection, so the cycle is one-way
        // and a single drain pair suffices. The inbound EventBus `loro`
        // consumer was removed, so it is NOT waited on here.
        {
            use tracing::Instrument;
            // Per-step settle barriers. Timeouts sized for Full+atomic-editor PBT runs
            // where BulkExternalAdd produces bursts the mirror applies serially:
            // 500ms wasn't enough to land all create events, leaving subsequent TypeChars
            // dispatched against blocks not-yet-in-the-Loro-tree (silent-drop in
            // `headless_editor_mirror.rs` because `editable_text(...)` returned Err).
            async {
                tokio::task::yield_now().await;
                self.ctx
                    .wait_for_loro_quiescence(std::time::Duration::from_secs(2))
                    .await;
                // Phase 5 (Option A): the event-bus per-consumer ack watermark
                // is gone (it was test-only scaffolding). Settle the
                // file/directory CDC the same way the quiescence assert
                // measures it — wait until the Turso CDC emission watermark is
                // stable — so legitimate dir/file CDC lands inside `target_seq`
                // rather than looking like post-settlement churn. Block/Loro
                // settle is covered by `wait_for_live_data_mirrors` + Loro
                // quiescence + the org idle/mtime gates.
                self.ctx
                    .wait_for_cdc_quiescent(
                        crate::test_environment::pbt_quiet_floor(),
                        std::time::Duration::from_secs(5),
                    )
                    .await;
                self.ctx
                    .wait_for_loro_quiescence(std::time::Duration::from_secs(2))
                    .await;
                tokio::task::yield_now().await;
                self.drain_cdc_events().await;
                self.drain_region_cdc_events().await;
                self.wait_for_live_data_mirrors(std::time::Duration::from_secs(2))
                    .await;
            }
            .instrument(tracing::info_span!("pbt.pre_inv16_settle"))
            .await;
        }

        // inv-editable-text-has-draggable: After draining, no more CDC events should arrive.
        {
            use tracing::Instrument;
            async {
                self.assert_cdc_quiescent().await;
            }
            .instrument(tracing::info_span!("pbt.assert_cdc_quiescent"))
            .await;
        }
    }

    /// Drain every instantiated `LiveData` mirror up to the current CDC
    /// emission watermark. Closes the race where the CDC emission has settled
    /// (`wait_for_cdc_quiescent`) but a mirror's `spawn_actor`
    /// task hasn't yet polled the matching CDC batches off its broadcast
    /// receiver — invariants would then read a stale snapshot (most visibly,
    /// invariant 8's region focus_roots check seeing the previous focus's
    /// children alongside the current ones).
    ///
    /// Sampling `cdc_emitted_watermark()` AFTER `wait_for_cdc_quiescent` is
    /// deliberate: by then every batch the transition could possibly produce
    /// has been stamped with a `seq`, so once each mirror's `consumed_seq`
    /// catches that watermark we know it has applied every CDC batch the
    /// matview emitted before this call.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_live_data_mirrors")]
    async fn wait_for_live_data_mirrors(&self, timeout: std::time::Duration) {
        // Pre-startup transitions (e.g. `WriteOrgFile` before `StartApp`)
        // run through this drain block too, but the engine doesn't exist
        // yet — and there can't be any LiveData mirrors either.
        if !self.ctx.is_running() {
            return;
        }
        self.cdc.wait_quiescent(timeout).await;
    }
}
