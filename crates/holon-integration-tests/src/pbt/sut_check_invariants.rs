//! `apply_transition_async` + the per-transition settle scaffolding
//! (the SplitBlock barrier in `block_tree_post_action`, the CDC drains, the
//! `assert_cdc_quiescent` barrier). The CDC-driven `LiveData` block/focus-root
//! mirror accessors were removed in E3 with `SutBackend` (their only reader).
//!
//! Invariant checking itself lives entirely in `run_invariant_registry`
//! (`pbt/invariant_runner.rs`): the registry runner owns the doc-URI
//! resolution, the `block_raw` convergence wait, and every registered body.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use holon_api::entity_uri::EntityUri;

use super::reference_state::ReferenceState;
use super::sut::E2ESut;

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
            E2ETransition::UndoLastMutation(_) | E2ETransition::Redo(_) => {
                // Block-convergence settle relocated here from the actions
                // (`apply_undo_last_mutation` / `apply_redo`, SutHandle decomposition
                // #1b): undo/redo restore the prior block set, so wait for the SUT to
                // reconverge to the ref's expected ids before invariants read. This
                // is `ref_state`-dependent, so it lives in the harness seam (which
                // owns `ref_state`), letting the cap actions be pure `&self`.
                let expected_ids = self.expected_block_ids(ref_state);
                self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
                    .await;
            }
            E2ETransition::PressKey(pk) => {
                // Relocated verbatim from `apply_press_key`'s `if has_enter` tail
                // (SutHandle decomposition): the Enter-split barrier + synthetic-id
                // reconcile + focus-handoff verify + caret park are all
                // `ref_state`-dependent, so they live in this harness seam (which
                // owns `ref_state`), letting the action be a pure `&self` keystroke
                // send. Enter dispatches `split_block`, which materializes a fresh
                // UUID for the suffix block — hand that back to the synthetic
                // `block::split-N` slot the ref-state allocated, mirroring
                // `apply_split_block`'s mapping step.
                use holon_api::Key;
                use holon_pbt_core::capabilities::{EngineFocus, SutDriver};
                let has_enter = pk.chord.0.iter().any(|k| matches!(k, Key::Enter));
                if has_enter {
                    // Turso barrier: let block_raw converge to the projected split
                    // row before the mapper reads it. The placeholder split id is
                    // treated as count-only by `wait_for_blocks_synced` (synthetic
                    // ids never reach CDC), so this converges as soon as the real
                    // split row lands — non-convergence surfaces in the mapper's
                    // count assert below. No-Turso's Loro split is synchronous — the
                    // mapper reads the snapshot directly, no barrier needed.
                    if matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
                        let expected_ids = self.expected_block_ids(ref_state);
                        let timeout = std::time::Duration::from_secs(5);
                        self.wait_for_blocks_synced(&expected_ids, timeout).await;
                    }
                    self.map_unmapped_split_synthetic_ids(ref_state, "[PressKey-Enter]")
                        .await;
                    // Prod's split sets focus + caret on the new block (caret 0) via
                    // the op response, applied in-process (ADR 0010). VERIFY the
                    // SUT's own focus landed where the ref expects before parking the
                    // mirror's caret — deriving the target from `ref_state` alone
                    // would re-impose the expected focus and mask a regressed focus
                    // handoff (the oracle-circularity the Jun-2026 review flagged).
                    // The caret seed itself (`home`) stays: the headless mirror
                    // tracks its caret independently and defaults to end-of-text.
                    if let Some(active) = ref_state.ui.tab.active_editor.as_ref() {
                        let expected_id = self.resolve_uri(&active.block_id);
                        // The op-response focus handoff (`apply_structural_focus`,
                        // ADR 0010) runs in the spawned dispatch task — block_raw
                        // converging (the barrier above) does NOT imply focus has
                        // moved yet. A single sample here raced that task and
                        // produced flaky "handoff DIVERGED, engine focused <old
                        // block>" failures (2026-06-11, window-active runs where the
                        // busier main thread widened the race window). Poll until
                        // convergence; the deadline keeps a genuinely regressed
                        // handoff loud.
                        let deadline =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                        loop {
                            match SutDriver::engine_focused_block(self).await {
                                EngineFocus::Focused(actual) => {
                                    if actual == expected_id {
                                        break;
                                    }
                                    if tokio::time::Instant::now() >= deadline {
                                        panic!(
                                            "[PressKey-Enter] split focus handoff DIVERGED: engine \
                                             focused {actual}, ref expects the new split block \
                                             {expected_id} (after 2s — async op-response focus \
                                             application never converged)"
                                        );
                                    }
                                }
                                EngineFocus::Unfocused => {
                                    if tokio::time::Instant::now() >= deadline {
                                        panic!(
                                            "[PressKey-Enter] split focus handoff LOST: engine has \
                                             no focused block, ref expects the new split block \
                                             {expected_id} (after 2s)"
                                        );
                                    }
                                }
                                // No frontend engine wired (SqlOnly headless): the
                                // op-response focus is unobservable here — disclosed,
                                // not silently skipped.
                                EngineFocus::NoEngine => {
                                    eprintln!(
                                        "[PressKey-Enter] split focus handoff UNVERIFIED \
                                         (no frontend engine); seeding caret on ref expectation \
                                         {expected_id}"
                                    );
                                    break;
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        }
                        self.sync_caret_to_new_split_block(&expected_id).await;
                    }
                }
            }
            E2ETransition::BulkExternalAdd(bea) => {
                // Relocated from `apply_bulk_external_add` (SutHandle decomposition):
                // the body serializes the FULL document from `ref_state`
                // (`resolve_ref_blocks`) — an action-time `ref_state` read — plus a
                // Turso DB-count verify, so it runs in this seam that owns
                // `ref_state`, letting the action be a `&self` no-op. The free fn
                // body is unchanged; only its call site moved here.
                crate::pbt::transitions::bulk_external_add::apply_bulk_external_add_to_sut(
                    self,
                    &bea.doc_uri,
                    &bea.blocks,
                    ref_state,
                )
                .await;
            }
            E2ETransition::ApplyMutation(am) => {
                // Relocated from `apply_apply_mutation` (SutHandle decomposition):
                // the dispatch resolves URIs/blocks from `ref_state` and the
                // LoroPeer path drives the `&mut self` `apply_peer_*` caps, so it
                // runs in this seam (which owns `ref_state` and is `&mut self`),
                // letting the action be a `&self` no-op. The free fn body is
                // unchanged; only its call site moved here.
                tracing::trace!("[apply] Applying mutation: {:?}", am.event.mutation);
                crate::pbt::transitions::apply_mutation::apply_apply_mutation_to_sut(
                    self,
                    am.event.clone(),
                    ref_state,
                )
                .await;
            }
            E2ETransition::StartApp(_) => {
                // Relocated from `apply_start_app` (SutAppLifecycle peel): the
                // pre-startup doc-uri reconcile (synthetic→resolved + `ctx.documents`
                // re-key) and the Turso seed-count settle are `ref_state`-derived,
                // so they run here after the action. The shared-Arc `doc_uri_map`
                // means the `LoroSut` installed during the action sees these inserts.
                for (synthetic_uri, filename) in &ref_state.files.documents {
                    if self.doc_uri_map.lock().unwrap().contains_key(synthetic_uri) {
                        continue;
                    }
                    if let Ok(resolved) = self.ctx.resolve_page_uri_by_name(filename).await {
                        self.doc_uri_map
                            .lock()
                            .unwrap()
                            .insert(synthetic_uri.clone(), resolved.clone());
                        let file_key = holon_api::EntityUri::file(filename);
                        let removed = self.ctx.documents.borrow_mut().remove(&file_key);
                        if let Some(path) = removed {
                            self.ctx.documents.borrow_mut().insert(resolved, path);
                        }
                    }
                }
                // Turso-only seed-count settle (no-Turso returns early in the action).
                if matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
                    let expected_ids = self.expected_block_ids(ref_state);
                    self.prime_seed_count(&expected_ids, Duration::from_secs(10))
                        .await;
                }
            }
            E2ETransition::SimulateRestart(_) => {
                // Block-convergence settle relocated here from
                // `apply_simulate_restart` (SutAppLifecycle peel): the action
                // re-parses the org files (no `ref_state`), then this seam waits
                // for the SUT to reconverge to the ref's expected ids. The cap
                // passes an empty expected-set so the inherent wait is a no-op.
                let expected_ids = self.expected_block_ids(ref_state);
                self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
                    .await;
            }
            E2ETransition::CreateDocument(cd) => {
                // Synthetic→uuid reconcile relocated here from
                // `apply_create_document` (SutAppLifecycle peel): the action mints
                // the real doc on disk (no `ref_state`); this seam re-derives the
                // minted uri via `resolve_page_uri_by_name` (the variant carries
                // `file_name`) and binds it to the ref's synthetic uri so later
                // transitions resolve the new doc.
                let uuid_uri = self
                    .ctx
                    .resolve_page_uri_by_name(&cd.file_name)
                    .await
                    .expect("CreateDocument post-action: minted doc URI not resolvable");
                let synthetic_uri = ref_state
                    .files
                    .documents
                    .iter()
                    .find(|(_, name)| *name == &cd.file_name)
                    .map(|(uri, _)| uri.clone())
                    .expect("CreateDocument: synthetic URI not found in reference state");
                self.doc_uri_map
                    .lock()
                    .unwrap()
                    .insert(synthetic_uri, uuid_uri);
            }
            _ => {}
        }
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
                // settle is covered by the Turso CDC quiescence wait below + Loro
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
}
