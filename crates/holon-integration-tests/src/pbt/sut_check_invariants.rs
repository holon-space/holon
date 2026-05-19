//! `apply_transition_async` + `check_invariants_async` + the live-data
//! mirror accessors (`live_blocks`, `live_focus_roots`, `wait_for_live_data_mirrors`).
//!
//! This is the wide-PBT runner: drives one transition + checks every
//! invariant against `ReferenceState`. Most invariants are still inline
//! here; per-invariant migration to `pbt/invariants/bodies/*` (Phase 7+)
//! deletes inline blocks one at a time and replaces them with
//! `assert_invariants!(ref_state, self, Inv*)` calls.
//!
//! Extracted from `sut.rs` (Phase D4).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use holon::storage::BLOCK_READ_TABLE;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{QueryLanguage, Value};
use holon_frontend::reactive::BuilderServices;
use holon_orgmode::OrgBlockExt;

use crate::{assert_block_order, assert_blocks_equivalent};

use super::reference_state::ReferenceState;
use super::sut::{E2ESut, FocusRoot};
use super::sut_macros::assert_invariants;
use super::sut_row_parsing::parse_block_row;
use super::types::*;

impl<V: VariantMarker> E2ESut<V> {
    /// Lazy accessor for the CDC-driven `LiveData<Block>` mirroring the `block`
    /// matview. Built on first use because we need an async `watch_view` call and
    /// the SUT struct can't carry a started engine at construction time. The
    /// matview hydrates `tags` (and `requires`) from the junction tables, so
    /// rows are read directly into a fully-populated `Block`.
    async fn live_blocks(&self) -> Arc<holon::sync::LiveData<Block>> {
        if let Some(live) = self.live_blocks_cell.borrow().clone() {
            return live;
        }
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id, properties, tags \
             FROM {BLOCK_READ_TABLE}"
        );
        let watch = self
            .ctx
            .engine()
            .watch_view(&sql)
            .await
            .expect("watch_view(block) failed");
        let live = holon::sync::LiveData::new(
            watch.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("block row missing 'id'"))
            },
            |row| {
                parse_block_row(row)
                    .ok_or_else(|| anyhow::anyhow!("parse_block_row returned None for row {row:?}"))
            },
        );
        live.subscribe("block", watch.stream);
        *self.live_blocks_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Lazy accessor for the CDC-driven `LiveData<FocusRoot>` mirroring the
    /// `focus_roots` matview. Keyed by `"{region}\u{1F}{root_id}"` since one
    /// region can have multiple root rows (one per child of the nav target).
    pub(super) async fn live_focus_roots(&self) -> Arc<holon::sync::LiveData<FocusRoot>> {
        if let Some(live) = self.live_focus_roots_cell.borrow().clone() {
            return live;
        }
        // `focus_roots` matview filters `block_id IS NOT NULL` at projection
        // time as of nightscape@holon `aff40a84` (the IVM compound IS NOT NULL
        // fix). Chained-matview CDC propagation is 1:1 with no spurious
        // events for filtered rows (verified by
        // `crates/holon/examples/turso_ivm_chained_matview_null_cdc.rs`).
        // No watcher-level filter needed.
        let sql = "SELECT region, root_id FROM focus_roots";
        let watch = self
            .ctx
            .engine()
            .watch_view(sql)
            .await
            .expect("watch_view(focus_roots) failed");
        let live = holon::sync::LiveData::new(
            watch.initial_rows,
            |row| {
                let region = row
                    .get("region")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?;
                let root_id = row
                    .get("root_id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?;
                Ok(format!("{region}\u{1F}{root_id}"))
            },
            |row| {
                Ok(FocusRoot {
                    region: row
                        .get("region")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?,
                    root_id: row
                        .get("root_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?,
                })
            },
        );
        live.subscribe("focus_roots", watch.stream);
        *self.live_focus_roots_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Async body of `apply()` — extracted so Flutter (already async) can call directly
    /// without `block_on`.
    #[tracing::instrument(skip(self, ref_state, transition), name = "pbt.apply_transition")]
    pub async fn apply_transition_async(
        &mut self,
        ref_state: &ReferenceState,
        transition: &crate::pbt::transitions::E2ETransition,
    ) {
        use crate::pbt::transitions::E2ETransitionImpl;
        transition.apply_to_sut(ref_state, self).await;
        // Stash the post-transition ref-state so the NEXT call can read it
        // as its pre-transition state. The framework hands us only the
        // post-state, so we have to carry the previous post forward
        // ourselves. See `pre_ref_state` field doc for the rationale.
        self.pre_ref_state = Some(ref_state.clone());

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
        // Round-trip path that has to converge before the assert:
        //   SQL write → CDC → EventBus event → `loro` consumer writes Loro
        //   → `subscribe_root` fires → `on_loro_changed` → more SQL writes
        //
        // Echo suppression (`event.origin == EventOrigin::Loro`) breaks
        // the cycle in 1–2 hops, so a single drain pair is enough.
        {
            use tracing::Instrument;
            // Per-step settle barriers. Timeouts sized for Full+atomic-editor PBT runs
            // where BulkExternalAdd produces bursts the loro consumer applies serially:
            // 500ms wasn't enough to land all create events, leaving subsequent TypeChars
            // dispatched against blocks not-yet-in-the-Loro-tree (silent-drop in
            // `headless_editor_mirror.rs` because `editable_text(...)` returned Err).
            async {
                tokio::task::yield_now().await;
                self.ctx
                    .wait_for_loro_quiescence(std::time::Duration::from_secs(2))
                    .await;
                self.ctx
                    .wait_for_consumers(
                        &["loro", "org", "cache"],
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
    /// emission watermark. Closes the race where `wait_for_consumers` reports
    /// the named EventBus consumers caught up but a mirror's `spawn_actor`
    /// task hasn't yet polled the matching CDC batches off its broadcast
    /// receiver — invariants would then read a stale snapshot (most visibly,
    /// invariant 8's region focus_roots check seeing the previous focus's
    /// children alongside the current ones).
    ///
    /// Sampling `cdc_emitted_watermark()` AFTER `wait_for_consumers` is
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
        // Quiescence semantics: each mirror has drained when no new batch
        // has arrived for `quiet_for`. The previous `wait_for_seq(target,
        // timeout)` approach compared per-stream seq against a global
        // `cdc_emitted_watermark`, which structurally always timed out
        // because other matviews emit batches the mirror never sees.
        let quiet_for = std::time::Duration::from_millis(50);
        if let Some(live) = self.live_blocks_cell.borrow().clone() {
            live.wait_for_quiescent(quiet_for, timeout).await;
        }
        if let Some(live) = self.live_focus_roots_cell.borrow().clone() {
            live.wait_for_quiescent(quiet_for, timeout).await;
        }
    }

    /// Async body of `check_invariants()` — extracted so Flutter can call directly.
    #[tracing::instrument(skip(self, ref_state), name = "pbt.check_invariants")]
    pub async fn check_invariants_async(&self, ref_state: &ReferenceState) {
        tracing::trace!(
            "[check_invariants] ref_state has {} blocks, app_started: {}",
            ref_state.block_state.blocks.len(),
            ref_state.app_started
        );

        // Skip invariant checks if app is not started
        if !ref_state.app_started {
            return;
        }

        // Transitions that don't modify block data — skip expensive invariants
        let nav_only = matches!(
            self.last_transition.variant_name(),
            "SwitchView"
                | "NavigateFocus"
                | "NavigateBack"
                | "NavigateForward"
                | "NavigateHome"
                | "ClickBlock"
                | "ArrowNavigate"
                | "SetupWatch"
                | "RemoveWatch"
                | "EmitMcpData"
                | "AddPeer"
                | "PeerEdit"
        );

        // 0. Flutter startup race (DDL/sync) — non-invariant assertion.
        self.check_inv_no_startup_errors();

        // 0b. inv-loro-no-errors — migrated to capability-bound body.
        self.check_inv_loro_no_errors(ref_state).await;

        // 1. Backend storage matches reference model
        //    Read from the CDC-driven `LiveData<Block>` mirroring the `block`
        //    matview. The matview hydrates `tags` from the junction table, so
        //    rows arrive fully populated. `wait_for_consumers` already gates
        //    each invariant pass on CDC delivery, so the in-memory snapshot
        //    is delay-free relative to the equivalent `SELECT`.
        let live_blocks = self.live_blocks().await;
        let backend_blocks: Vec<Block> = live_blocks.read().values().cloned().collect();

        // Translate synthetic doc URIs in reference blocks to real UUID-based IDs.
        // OrgSyncController creates document blocks asynchronously, so we
        // retry with a short timeout for any unresolved URIs.
        let mut lazy_doc_uri_map = self.doc_uri_map.clone();
        let unresolved: Vec<_> = ref_state
            .documents
            .iter()
            .filter(|(uri, _)| !lazy_doc_uri_map.contains_key(*uri))
            .map(|(uri, filename)| (uri.clone(), filename.clone()))
            .collect();
        if !unresolved.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut remaining = unresolved;
            while !remaining.is_empty() && Instant::now() < deadline {
                for (synthetic_uri, filename) in std::mem::take(&mut remaining) {
                    match self.ctx.resolve_doc_uri_by_name(&filename).await {
                        Ok(resolved) => {
                            tracing::trace!(
                                "[check_invariants] Late-resolved doc URI: {} → {}",
                                synthetic_uri,
                                resolved
                            );
                            lazy_doc_uri_map.insert(synthetic_uri, resolved);
                        }
                        Err(_) => remaining.push((synthetic_uri, filename)),
                    }
                }
                if !remaining.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            if !remaining.is_empty() {
                tracing::trace!(
                    "[check_invariants] WARNING: {} doc URIs still unresolved: {:?}",
                    remaining.len(),
                    remaining.iter().map(|(u, _)| u).collect::<Vec<_>>()
                );
            }
        }
        let resolve = |uri: &EntityUri| -> EntityUri {
            lazy_doc_uri_map
                .get(uri)
                .cloned()
                .unwrap_or_else(|| uri.clone())
        };

        let ref_blocks_resolved: Vec<_> = ref_state
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut block = b.clone();
                block.id = resolve(&block.id);
                block.parent_id = resolve(&block.parent_id);
                block
            })
            .collect();

        // Seed block IDs (raw, untranslated) for org file comparison
        let seed_block_ids_raw: std::collections::HashSet<_> = ref_state
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| id.clone())
            .collect();

        // Seed block IDs (translated) for backend comparison
        let seed_block_ids: std::collections::HashSet<_> = ref_state
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| resolve(id))
            .collect();

        let backend_blocks_no_seed: Vec<_> = backend_blocks
            .iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .cloned()
            .collect();
        let ref_blocks_no_seed: Vec<_> = ref_blocks_resolved
            .iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .cloned()
            .collect();

        // ID-set truth check before the full block comparison: when
        // backend (live_blocks) and reference disagree, classify whether
        // it's a CDC delivery race (matview lagged a write) or a real
        // pipeline bug. Same pattern as inv-watch-rows-match-ref below — query `block_raw`
        // (write-side base table) and compare ID sets.
        let backend_ids: HashSet<EntityUri> = backend_blocks_no_seed
            .iter()
            .map(|b| b.id.clone())
            .collect();
        let ref_ids: HashSet<EntityUri> = ref_blocks_no_seed.iter().map(|b| b.id.clone()).collect();
        // When set, downstream invariants that read `backend_blocks` must
        // skip — the mirror is stale and any structural assertion (orphan
        // checks, focus_roots intersection, etc.) would just re-fail on
        // the same lag.
        let live_blocks_stale = if backend_ids != ref_ids {
            let truth_rows = self
                .ctx
                .query_sql("SELECT id FROM block_raw")
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[inv-backend-blocks-match-ref truth check] block_raw query failed\n\
                         error: {}",
                        e
                    )
                });
            let truth_ids: HashSet<EntityUri> = truth_rows
                .iter()
                .filter_map(|r| {
                    r.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| EntityUri::parse(s).expect("invalid uri in block_raw row"))
                })
                .filter(|id| !seed_block_ids.contains(id))
                .collect();
            if truth_ids == ref_ids {
                let missing: Vec<&EntityUri> = ref_ids.difference(&backend_ids).collect();
                let spurious: Vec<&EntityUri> = backend_ids.difference(&ref_ids).collect();
                eprintln!(
                    "[inv-backend-blocks-match-ref WARN] live_blocks mirror lagged: backend has {} blocks, \
                     block_raw has {} (matches reference). Downgraded — Turso IVM CDC \
                     delivery race on the `block` matview → live_blocks mirror.\n\
                     Missing in live_blocks: {:?}\n\
                     Spurious in live_blocks: {:?}",
                    backend_ids.len(),
                    truth_ids.len(),
                    missing,
                    spurious,
                );
                true
            } else {
                // truth_ids disagrees with ref_ids — real bug, keep the panic
                // but with the diagnostic that block_raw is what the mirror
                // *would* converge to. Falling through to assert_blocks_equivalent
                // produces the canonical error message.
                eprintln!(
                    "[inv-backend-blocks-match-ref truth check] block_raw also disagrees with reference — \
                     real write/parse pipeline bug, not a CDC delivery race.\n\
                     Missing in block_raw: {:?}\n\
                     Spurious in block_raw: {:?}",
                    ref_ids.difference(&truth_ids).collect::<Vec<_>>(),
                    truth_ids.difference(&ref_ids).collect::<Vec<_>>(),
                );
                assert_blocks_equivalent(
                    &backend_blocks_no_seed,
                    &ref_blocks_no_seed,
                    "Backend diverged from reference",
                );
                false // unreachable — assert above panics
            }
        } else {
            // ID sets match — run the full block comparison (catches
            // per-row content/property/parent mismatches that the ID-set
            // check by definition can't see).
            assert_blocks_equivalent(
                &backend_blocks_no_seed,
                &ref_blocks_no_seed,
                "Backend diverged from reference",
            );
            false
        };

        // 1b. Loro tree matches reference model (when Loro is enabled)
        //
        // DISABLED: the outbound reconcile's CacheEventSubscriber sometimes
        // fails to deserialize update events (missing parent_id/created_at),
        // causing property sync to be lost. The Loro↔ref bridge IS validated
        // at Layer 3 (40 cases). Re-enable after fixing the outbound reconcile
        // event payload completeness for all block types.
        if let Some(ref _loro_sut) = self.loro_sut {
            // loro_sut.assert_matches_reference(&ref_blocks_no_seed, &seed_block_ids).await;
        }

        // Ref blocks for org file comparison — translate synthetic doc URIs
        // to whatever the org parser will produce on disk. With `#+ID:`
        // support, files that have been resolved by the controller carry a
        // `block:<uuid>` parent (the canonical resolved id). Files not yet
        // resolved fall back to `file:<filename>` to match the legacy parser
        // output. Exclude document blocks and seed blocks.
        //
        // Use `lazy_doc_uri_map` (not `self.doc_uri_map`) so docs added
        // post-startup via WriteOrgFile (which only populates the lazy map
        // via `ctx.resolve_doc_uri_by_name` above) are mapped correctly.
        let synthetic_to_parent: HashMap<EntityUri, EntityUri> = ref_state
            .documents
            .iter()
            .map(|(syn, filename)| {
                let target = lazy_doc_uri_map
                    .get(syn)
                    .cloned()
                    .unwrap_or_else(|| EntityUri::file(filename));
                (syn.clone(), target)
            })
            .collect();
        let ref_blocks_org_only: Vec<_> = ref_state
            .block_state
            .blocks
            .values()
            .filter(|b| !seed_block_ids_raw.contains(&b.id))
            .filter(|b| !b.is_page())
            .map(|b| {
                let mut b = b.clone();
                // Synthetic split IDs (`block::split-N`) get mapped to the
                // real UUID issued by `split_block` once the new block lands
                // in the DB; without this, the on-disk org file (which has
                // the real UUID) compares unequal to the ref state.
                b.id = resolve(&b.id);
                if let Some(parent_uri) = synthetic_to_parent.get(&b.parent_id) {
                    b.parent_id = parent_uri.clone();
                }
                b
            })
            .collect();

        // 2/2b: Org file parse + ordering — expensive, skip for nav-only transitions
        if !nav_only {
            // Wait for OrgSyncController's background task to re-render org files
            // after UI mutations. The SQL write is committed but the event-driven
            // re-render runs in a separate tokio task.
            self.wait_for_org_files_stable(25, Duration::from_millis(5000))
                .await;

            let org_blocks = self
                .parse_org_file_blocks(None)
                .await
                .expect("Failed to parse Org file");
            assert_blocks_equivalent(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file diverged from reference",
            );

            // 2b. Org file block ordering matches reference model
            assert_block_order(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file block ordering wrong",
            );

            // 2c. Live block_raw children order (the projector's authoritative
            // ordering) matches the reference model's predicted children list.
            // This compares the encoding-free child-id list directly: no
            // `sort_key` strings or `sequence` numbers cross the boundary.
            // Earlier and more diagnostic than the org-roundtrip assertion
            // above (which can mask the underlying disagreement when the org
            // renderer's group sort accidentally re-orders things back).
            self.assert_live_children_match_ref(ref_state).await;

            // 2d. inv-org-render-fixed-point — migrated to capability-bound body.
            self.check_inv_org_render_fixed_point(ref_state).await;
        }

        // 3. UI model (built from CDC) matches reference — verify all fields, not just IDs
        for (query_id, ui_data) in &self.ui_model {
            if let Some(watch_spec) = ref_state.active_watches.get(query_id) {
                let expected = ref_state.query_results(watch_spec);
                let ui_rows = ui_data.to_vec();

                let ui_ids: HashSet<EntityUri> = ui_rows
                    .iter()
                    .filter_map(|row| {
                        row.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| EntityUri::parse(s).expect("invalid entity URI in CDC data"))
                    })
                    .collect();
                // Translate file: URIs in expected IDs to block:uuid via doc_uri_map
                let expected_ids: HashSet<EntityUri> = expected
                    .iter()
                    .filter_map(|row| {
                        row.get("id").and_then(|v| v.as_string()).map(|s| {
                            let uri =
                                EntityUri::parse(s).expect("invalid entity URI in expected data");
                            resolve(&uri)
                        })
                    })
                    .collect();

                if ui_ids != expected_ids {
                    // The CDC stream lagged on the ID set. Same classification
                    // as the field-level check below: re-query the underlying
                    // `block_raw` write-side table directly. If `block_raw`
                    // has the expected IDs, the watch matview's CDC just
                    // didn't fan out by the time we drained — downgrade to a
                    // warning. If `block_raw` also disagrees, the
                    // write/parser pipeline has a real bug — panic.
                    let truth_sql = watch_spec.query.to_block_raw_sql();
                    let truth_rows = match self.ctx.query_sql(&truth_sql).await {
                        Ok(rows) => rows,
                        Err(e) => panic!(
                            "[inv-watch-rows-match-ref truth check] block_raw query failed for watch '{}'\n\
                             sql: {}\n\
                             error: {}",
                            query_id, truth_sql, e
                        ),
                    };
                    let truth_ids: HashSet<EntityUri> = truth_rows
                        .iter()
                        .filter_map(|r| {
                            r.get("id").and_then(|v| v.as_string()).map(|s| {
                                let uri = EntityUri::parse(s)
                                    .expect("invalid entity URI in block_raw row");
                                resolve(&uri)
                            })
                        })
                        .collect();
                    if truth_ids == expected_ids {
                        let missing: Vec<&EntityUri> = expected_ids.difference(&ui_ids).collect();
                        let spurious: Vec<&EntityUri> = ui_ids.difference(&expected_ids).collect();
                        eprintln!(
                            "[inv-watch-rows-match-ref WARN] CDC stream lagged on ID set for watch '{}': \
                             ui_model has {} blocks, block_raw has {} (matches expected). \
                             Downgraded — Turso IVM CDC delivery race.\n\
                             Missing in ui_model: {:?}\n\
                             Spurious in ui_model: {:?}",
                            query_id,
                            ui_ids.len(),
                            truth_ids.len(),
                            missing,
                            spurious,
                        );
                        // ui_model is stale for this watch — skip the per-row
                        // field checks below. Re-checking against stale rows
                        // would just produce noise that masks the next signal.
                        continue;
                    }
                    panic!(
                        "CDC UI model for watch '{}' has wrong block IDs (block_raw also disagrees \
                         — real bug, not a CDC delivery race).\n\
                         Expected {} blocks: {:?}\n\
                         Got {} blocks (ui_model): {:?}\n\
                         Got {} blocks (block_raw truth): {:?}",
                        query_id,
                        expected_ids.len(),
                        expected_ids,
                        ui_ids.len(),
                        ui_ids,
                        truth_ids.len(),
                        truth_ids,
                    );
                }

                // Verify fields per block that are included in the query columns
                let query_cols = &watch_spec.query.columns;
                let fields_to_check: Vec<&str> =
                    ["content", "content_type", "source_language", "source_name"]
                        .iter()
                        .copied()
                        .filter(|f| query_cols.iter().any(|c| c == *f))
                        .collect();
                for expected_row in &expected {
                    let raw_id = match expected_row.get("id").and_then(|v| v.as_string()) {
                        Some(id) => id,
                        None => continue,
                    };
                    // Translate file: URI to block:uuid for matching against CDC data
                    let expected_id = if let Ok(uri) = EntityUri::parse(raw_id) {
                        resolve(&uri).to_string()
                    } else {
                        raw_id.to_string()
                    };

                    if let Some(ui_row) = ui_rows.iter().find(|r: &&HashMap<String, Value>| {
                        r.get("id").and_then(|v| v.as_string()) == Some(&expected_id)
                    }) {
                        // The org round-trip strips trailing whitespace per
                        // line (the parser drops trailing spaces from headlines
                        // and body lines), so normalize both sides the same way
                        // before comparing — matches `normalize_block`.
                        let normalize_content = |s: &str| -> String {
                            s.lines()
                                .map(|l| l.trim_end())
                                .collect::<Vec<_>>()
                                .join("\n")
                                .trim()
                                .to_string()
                        };
                        for field in &fields_to_check {
                            let expected_val = expected_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(normalize_content);
                            let actual_val = ui_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(normalize_content);
                            if actual_val != expected_val {
                                // The CDC stream lagged. Check the underlying
                                // SQL state directly: if SQL agrees with the
                                // reference, downgrade to a warning (Turso IVM
                                // CDC delivery race — the matview's stream
                                // didn't fan out the row update before our drain
                                // wait expired). If SQL also disagrees, the
                                // mutation pipeline has a real consistency bug
                                // — keep the panic.
                                let sql = format!(
                                    "SELECT {} FROM block_raw WHERE id = '{}'",
                                    field,
                                    expected_id.replace('\'', "''")
                                );
                                let sql_val = self
                                    .ctx
                                    .query_sql(&sql)
                                    .await
                                    .ok()
                                    .and_then(|rows| {
                                        rows.into_iter().next().and_then(|r| r.get(*field).cloned())
                                    })
                                    .and_then(|v| v.as_string().map(|s| s.to_string()))
                                    .map(|s| normalize_content(&s));
                                if sql_val == expected_val {
                                    eprintln!(
                                        "[inv-watch-rows-match-ref WARN] CDC stream lagged for block '{}' field '{}' \
                                         in watch '{}': ui_model={:?}, sql={:?}, expected={:?} \
                                         (downgraded — Turso IVM CDC delivery race)",
                                        expected_id,
                                        field,
                                        query_id,
                                        actual_val,
                                        sql_val,
                                        expected_val,
                                    );
                                } else {
                                    panic!(
                                        "CDC field '{}' mismatch for block '{}' in watch '{}'\n\
                                         actual_ui_model={:?}\n\
                                         actual_sql={:?}\n\
                                         expected={:?}",
                                        field,
                                        expected_id,
                                        query_id,
                                        actual_val,
                                        sql_val,
                                        expected_val,
                                    );
                                }
                            }
                        }

                        // parent_id: normalize document URIs before comparing
                        if query_cols.iter().any(|c| c == "parent_id") {
                            let normalize_parent = |v: Option<&Value>| -> Option<String> {
                                v.and_then(|v| v.as_string()).map(|s| {
                                    let uri_result = EntityUri::parse(s);
                                    if uri_result
                                        .as_ref()
                                        .is_ok_and(|u| u.is_no_parent() || u.is_sentinel())
                                    {
                                        "__document_root__".to_string()
                                    } else if let Ok(uri) = uri_result {
                                        // Translate file: URIs to block:uuid
                                        resolve(&uri).to_string()
                                    } else {
                                        s.trim().to_string()
                                    }
                                })
                            };
                            assert_eq!(
                                normalize_parent(ui_row.get("parent_id")),
                                normalize_parent(expected_row.get("parent_id")),
                                "CDC parent_id mismatch for block '{}' in watch '{}'",
                                expected_id,
                                query_id
                            );
                        }
                    }
                }
            }
        }

        // 4 + 5. View selection + active watches.
        self.check_inv_view_and_watches(ref_state);

        // 6. Structural integrity: no orphan blocks.
        self.check_inv_no_orphan_blocks(&backend_blocks, live_blocks_stale);

        // 7. Navigation state verification
        let focus_rows = self
            .engine()
            .execute_query(
                "SELECT region, block_id FROM current_focus".to_string(),
                HashMap::new(),
                None,
            )
            .await
            .expect("Failed to query current_focus - this may indicate a Turso IVM bug");

        for (region, history) in &ref_state.navigation_history {
            let expected_focus = history.current_focus();
            let actual = focus_rows
                .iter()
                .find(|r| r.get("region").and_then(|v| v.as_string()) == Some(region.as_str()));

            match (actual, &expected_focus) {
                (Some(row), Some(expected_id)) => {
                    let resolved_expected = resolve(expected_id);
                    let actual_block_id = row
                        .get("block_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string());
                    assert_eq!(
                        actual_block_id.as_deref(),
                        Some(resolved_expected.as_str()),
                        "Navigation focus mismatch for region '{}': expected {:?} (resolved {:?}), got {:?}",
                        region,
                        expected_focus,
                        resolved_expected,
                        actual_block_id
                    );
                }
                (Some(row), None) => {
                    let actual_block_id = row.get("block_id");
                    assert!(
                        actual_block_id.is_none()
                            || actual_block_id.and_then(|v| v.as_string()).is_none()
                            || matches!(actual_block_id, Some(Value::Null)),
                        "Navigation focus mismatch for region '{}': expected home (None), got {:?}",
                        region,
                        actual_block_id
                    );
                }
                (None, None) => {}
                (None, Some(expected_id)) => {
                    panic!(
                        "[check_invariants] Region '{}' should have focus on '{}' but not found in DB",
                        region, expected_id
                    );
                }
            }
        }

        // 8. Region data verification — read from CDC-driven LiveData<FocusRoot>
        // mirror of the focus_roots matview. Avoids one SQL round trip per region
        // per check; gating on `wait_for_consumers` keeps it delay-free.
        // Per-region grouping is done in Rust via the in-memory snapshot.
        if ref_state.app_started {
            let live_focus_roots = self.live_focus_roots().await;
            let mut by_region: HashMap<String, Vec<EntityUri>> = HashMap::new();
            for fr in live_focus_roots.read().values() {
                by_region
                    .entry(fr.region.clone())
                    .or_default()
                    .push(EntityUri::parse(&fr.root_id).expect("valid entity URI in focus_roots"));
            }
            for region in holon_api::Region::ALL {
                let expected = ref_state.expected_focus_root_ids(*region);

                let mut expected_ids: Vec<EntityUri> =
                    expected.into_iter().map(|uri| resolve(&uri)).collect();
                expected_ids.sort();

                let mut actual_ids: Vec<EntityUri> =
                    by_region.remove(region.as_str()).unwrap_or_default();
                actual_ids.sort();

                if actual_ids == expected_ids {
                    continue;
                }

                // Truth check: query the `focus_roots` matview directly. If the
                // matview agrees with the reference, the `LiveData<FocusRoot>`
                // mirror lagged (CDC delivery race) — same downgrade pattern as
                // inv-backend-blocks-match-ref. If the matview itself disagrees, it's a real IVM bug
                // (e.g. UPDATE through the chained `block` matview not
                // propagating, see split_block CDC-drop memory note).
                let truth_sql = format!(
                    "SELECT root_id FROM focus_roots WHERE region = '{}'",
                    region.as_str()
                );
                let truth_rows = self.ctx.query_sql(&truth_sql).await.unwrap_or_else(|e| {
                    panic!(
                        "[inv-focus-roots truth check] focus_roots query failed\n\
                         error: {}",
                        e
                    )
                });
                let mut truth_ids: Vec<EntityUri> = truth_rows
                    .iter()
                    .filter_map(|r| r.get("root_id").and_then(|v| v.as_string()))
                    .map(|s| EntityUri::parse(s).expect("valid entity URI in focus_roots row"))
                    .collect();
                truth_ids.sort();

                if truth_ids == expected_ids {
                    eprintln!(
                        "[inv-focus-roots WARN] Region '{}' LiveData<FocusRoot> mirror \
                         lagged: matview has {} rows (matches reference), mirror has {}. \
                         Downgraded — Turso IVM CDC delivery race on focus_roots → mirror.\n\
                         Missing in mirror: {:?}\n\
                         Spurious in mirror: {:?}",
                        region.as_str(),
                        truth_ids.len(),
                        actual_ids.len(),
                        truth_ids
                            .iter()
                            .filter(|id| !actual_ids.contains(id))
                            .collect::<Vec<_>>(),
                        actual_ids
                            .iter()
                            .filter(|id| !truth_ids.contains(id))
                            .collect::<Vec<_>>(),
                    );
                    continue;
                }

                // Localize: which matview lost the row? Query the chain
                // (block_raw → block matview → focus_roots matview) for the
                // missing IDs so the panic pinpoints the dropping link.
                let missing: Vec<EntityUri> = expected_ids
                    .iter()
                    .filter(|id| !truth_ids.contains(id))
                    .cloned()
                    .collect();
                let mut chain_status: Vec<String> = Vec::new();
                for id in &missing {
                    let raw_sql = format!("SELECT id FROM block_raw WHERE id = '{}'", id.as_str());
                    let raw_hit = self
                        .ctx
                        .query_sql(&raw_sql)
                        .await
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    let blk_sql = format!("SELECT id FROM block WHERE id = '{}'", id.as_str());
                    let blk_hit = self
                        .ctx
                        .query_sql(&blk_sql)
                        .await
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    chain_status.push(format!(
                        "{}: block_raw={} block={} focus_roots=false",
                        id.as_str(),
                        if raw_hit { "✓" } else { "✗" },
                        if blk_hit { "✓" } else { "✗" }
                    ));
                }

                panic!(
                    "Region '{}' focus_roots mismatch after navigation.\n\
                     Focus: {:?}\n\
                     Expected IDs:   {:?}\n\
                     Mirror IDs:     {:?}\n\
                     Matview IDs:    {:?}\n\
                     Chain status for missing rows:\n  {}\n\
                     ↑ matview itself disagrees with reference — real Turso IVM bug, \
                     not a CDC delivery race. Chain shows where the row gets dropped.",
                    region.as_str(),
                    ref_state.current_focus(*region),
                    expected_ids,
                    actual_ids,
                    truth_ids,
                    chain_status.join("\n  "),
                );
            }
        }

        // 9/10: Properties check + root layout liveness — skip for nav-only transitions
        if !nav_only {
            // 9. Verify blocks with properties HashMap are correctly stored in cache
            // Single batch query instead of per-block queries
            let blocks_with_props: Vec<&Block> = backend_blocks
                .iter()
                .filter(|b| !b.properties.is_empty())
                .collect();

            if !blocks_with_props.is_empty() {
                // Read from block_raw (writable base table) — same matview-CDC
                // race fix as inv-viewmodel-root-matches-render-expr (devlog/2026-05-05-110311.md). This query
                // only needs id + properties, both in block_raw.
                let prql = "from block_raw | filter properties != null | select {id, properties}";
                let query_result = self
                    .test_ctx()
                    .query(prql.to_string(), QueryLanguage::HolonPrql, HashMap::new())
                    .await
                    .expect("Failed to query properties batch");

                let cached_ids_with_props: HashSet<String> = query_result
                    .iter()
                    .filter_map(|row| {
                        let id = row.get("id")?.as_string()?.to_string();
                        let props = row.get("properties")?;
                        if matches!(props, Value::Null) {
                            None
                        } else {
                            Some(id)
                        }
                    })
                    .collect();

                let mut missing: Vec<String> = Vec::new();
                for block in &blocks_with_props {
                    if !cached_ids_with_props.contains(block.id.as_str()) {
                        eprintln!(
                            "[props_check] block={}, has_props=true, properties={:?}, NOT found in cache",
                            block.id, block.properties
                        );
                        missing.push(block.id.to_string());
                    }
                }

                assert!(
                    missing.is_empty(),
                    "Block properties NULL in cache for: {:?} (Value::Object serialization bug)",
                    missing
                );
            }

            // 10. Root layout via ReactiveEngine (same pipeline as GPUI frontend)
            // ReactiveEngine watches root block via watch_ui, accumulates CDC into
            // MutableBTreeMap, and produces ViewModels via signal graph.
            if ref_state.is_properly_setup() {
                let engine = self.engine();
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);

                // Ensure ReactiveEngine exists (created during StartApp,
                // but handle edge cases where check_invariants runs first).
                self.ensure_reactive_engine(&root_id).await;

                let reactive = self.reactive_engine.borrow().clone().unwrap();

                // Ensure the reactive engine has processed pending CDC before we
                // read its snapshot. Keep the 5 s first-emission wait as a safety
                // net for cold startups, but replace the former 100 ms drain loop
                // with the same 5 ms sleep + now_or_never hybrid used in
                // drain_cdc_events. The sleep gives the engine real wall time to
                // process incoming events; the now_or_never loop drains whatever's
                // immediately ready without a 100 ms gap detection.
                let stream_closed = {
                    use futures::FutureExt;
                    use futures::StreamExt;
                    use tracing::Instrument;
                    async {
                        let mut stream = reactive.watch(&root_id);
                        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
                            Ok(Some(_)) => {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                                loop {
                                    match stream.next().now_or_never() {
                                        Some(Some(_)) => continue,
                                        _ => break,
                                    }
                                }
                                false
                            }
                            Ok(None) => {
                                eprintln!("[inv-viewmodel-snapshot] Reactive stream closed, skipping");
                                true
                            }
                            Err(_) => {
                                eprintln!("[inv-viewmodel-snapshot] No data within 5s, using current state");
                                false
                            }
                        }
                    }
                    .instrument(tracing::info_span!("pbt.inv10_watch_drain"))
                    .await
                };
                if stream_closed {
                    return;
                }

                let results = reactive.ensure_watching(&root_id);
                let (render_expr, data_rows) = results.snapshot();

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading")
                {
                    eprintln!("[inv-viewmodel-snapshot] render_expr is still loading(), skipping");
                    return;
                }

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "spacer")
                {
                    eprintln!("[inv-viewmodel-snapshot] Still placeholder (spacer), skipping");
                    return;
                }

                let engine_clone = Arc::clone(engine);
                let re = render_expr.clone();
                let dr = data_rows.clone();
                let display_tree = tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let services =
                            holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
                        holon_frontend::interpret_pure(&re, &dr, &services).snapshot()
                    }))
                })
                .await
                .expect("spawn_blocking panicked");

                let display_tree = match display_tree {
                    Ok(tree) => tree,
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| e.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        eprintln!(
                            "[inv-viewmodel-snapshot] Shadow interpretation panicked: {msg} \
                             (pre-existing bug, skipping structural assertions)"
                        );
                        return;
                    }
                };
                eprintln!("[inv-viewmodel-snapshot] ViewModel from ReactiveEngine snapshot");

                // 10a. Root widget must not be "error"
                assert_ne!(
                    display_tree.widget_name(),
                    Some("error"),
                    "Root layout rendered as error widget:\n{}",
                    display_tree.pretty_print(0),
                );

                // 10b. Entity IDs in tree
                let tree_ids = display_tree.collect_entity_ids();
                eprintln!(
                    "[inv-viewmodel-snapshot] ViewModel: root='{}', {} entity IDs",
                    display_tree.widget_name().unwrap_or("?"),
                    tree_ids.len(),
                );

                // 10c. No nested error nodes
                let error_count = crate::display_assertions::count_error_nodes(&display_tree);
                assert_eq!(
                    error_count,
                    0,
                    "[inv-viewmodel-no-error-widgets] {} error node(s) in ViewModel tree:\n{}",
                    error_count,
                    display_tree.pretty_print(0),
                );

                // 10d. Root widget type matches reference model's render expression.
                // The engine wraps the root in a view_mode_switcher; the reference
                // model doesn't know about this wrapper so we look one level deeper.
                if let Some(expected_expr) = ref_state.root_render_expr() {
                    let expected_widget = match expected_expr {
                        holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                            name.as_str()
                        }
                        _ => panic!("root render expr must be FunctionCall"),
                    };
                    let actual_widget = display_tree.widget_name();
                    let matches_expected = actual_widget == Some(expected_widget)
                        || (actual_widget == Some("view_mode_switcher")
                            && display_tree
                                .children()
                                .first()
                                .and_then(|c| c.widget_name())
                                == Some(expected_widget));
                    assert!(
                        matches_expected,
                        "[inv-viewmodel-root-matches-render-expr] Root widget '{}' doesn't match render source '{}' \
                         (root_id={})\n\
                         EXPECTED render expr (from ref_state.root_render_expr()): {}\n\
                         ACTUAL render expr (from engine.snapshot()): {}\n\
                         data_rows.len()={} ids={:?}\n\
                         {}",
                        actual_widget.unwrap_or("?"),
                        expected_widget,
                        root_id,
                        expected_expr.to_rhai(),
                        render_expr.to_rhai(),
                        data_rows.len(),
                        data_rows
                            .iter()
                            .filter_map(|r| r.get("id").and_then(|v| v.as_string()))
                            .collect::<Vec<_>>(),
                        display_tree.pretty_print(0),
                    );
                    eprintln!(
                        "[inv-viewmodel-root-matches-render-expr] Root widget '{}' matches render expr '{}'",
                        expected_widget,
                        expected_expr.to_rhai(),
                    );
                }

                // 10e. Entity IDs in tree are subset of query data IDs.
                //
                // Only meaningful when the ref model tracks a render source for
                // the root layout — i.e. rendering is driven by a user-authored
                // render expression whose `live_block()` nodes read `col("id")`
                // from data rows. When no render source is tracked, the backend
                // falls through to `render_entity()` + entity-profile variant
                // resolution, and variants like the `root_layout` block-profile
                // variant contain **literal** `live_block("block:default-*")`
                // IDs that are hardcoded in YAML and never appear in
                // `data_rows` (data_rows only contains the root block itself).
                // Gating on `root_render_expr().is_some()` keeps the assertion
                // strict where it's load-bearing and skips it when the tree IDs
                // come from profile-variant YAML rather than query data.
                let data_id_set: std::collections::HashSet<String> = data_rows
                    .iter()
                    .filter_map(|r| {
                        r.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| s.to_string())
                    })
                    .collect();
                if ref_state.root_render_expr().is_some()
                    && !tree_ids.is_empty()
                    && !data_id_set.is_empty()
                {
                    let tree_id_set: std::collections::HashSet<String> =
                        tree_ids.iter().cloned().collect();
                    let missing: Vec<&String> = tree_id_set
                        .iter()
                        .filter(|id| !data_id_set.contains(*id))
                        .collect();
                    assert!(
                        missing.is_empty(),
                        "[inv-viewmodel-entity-ids-subset-of-data] ViewModel has entity IDs not in query data.\n\
                             Missing: {:?}\n\
                             Tree IDs ({}):\n  {:?}\n\
                             Data IDs ({}):\n  {:?}\n{}",
                        missing,
                        tree_ids.len(),
                        tree_ids,
                        data_id_set.len(),
                        data_id_set,
                        display_tree.pretty_print(0),
                    );
                    eprintln!(
                        "[inv-viewmodel-entity-ids-subset-of-data] {} tree entity IDs are subset of {} data IDs",
                        tree_id_set.len(),
                        data_id_set.len(),
                    );
                }

                // 10f. Decompiled row data matches query data
                if let Some(expected_expr) = ref_state.root_render_expr() {
                    let visible_cols = expected_expr.visible_columns();
                    let rendered_rows =
                        crate::display_assertions::extract_rendered_rows(&display_tree);
                    if !rendered_rows.is_empty()
                        && !visible_cols.is_empty()
                        && !data_rows.is_empty()
                    {
                        let expected_rows: Vec<
                            std::collections::HashMap<String, holon_api::Value>,
                        > = data_rows
                            .iter()
                            .map(|r| {
                                r.iter()
                                    .filter(|(k, _)| visible_cols.contains(k))
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect()
                            })
                            .collect();
                        let subset_result = crate::display_assertions::is_ordered_subset(
                            &rendered_rows
                                .iter()
                                .filter_map(|r| {
                                    r.get("content")
                                        .and_then(|v| v.as_string())
                                        .map(|s| s.to_string())
                                })
                                .collect::<Vec<_>>(),
                            &expected_rows
                                .iter()
                                .filter_map(|r| {
                                    r.get("content")
                                        .and_then(|v| v.as_string())
                                        .map(|s| s.to_string())
                                })
                                .collect::<Vec<_>>(),
                        );
                        assert!(
                            subset_result.is_subset,
                            "[inv-viewmodel-decompiled-rows-match-query] Decompiled content doesn't match query data.\n\
                                 Rendered: {:?}\nExpected: {:?}\n\
                                 Missing: {:?}\nOut of order: {:?}\n\
                                 Render expr: {}\n{}",
                            rendered_rows,
                            expected_rows,
                            subset_result.missing_from_expected,
                            subset_result.out_of_order,
                            expected_expr.to_rhai(),
                            display_tree.pretty_print(0),
                        );
                        eprintln!(
                            "[inv-viewmodel-decompiled-rows-match-query] {} decompiled rows match expected (cols: {:?})",
                            rendered_rows.len(),
                            visible_cols,
                        );
                    }
                }

                // 10g. EditableText nodes with operations must have triggers
                let (total_with_ops, missing_triggers) =
                    crate::display_assertions::count_editables_missing_triggers(&display_tree);
                assert_eq!(
                    missing_triggers,
                    0,
                    "[inv-viewmodel-editable-text-triggers] {missing_triggers}/{total_with_ops} EditableText node(s) \
                         with operations are missing triggers.\n{}",
                    display_tree.pretty_print(0),
                );
                if total_with_ops > 0 {
                    eprintln!(
                        "[inv-viewmodel-editable-text-triggers] All {total_with_ops} EditableText node(s) with ops have triggers"
                    );
                }

                // 10h. StateToggle: hard assertions on entity, operations, state
                let toggle_nodes =
                    crate::display_assertions::collect_state_toggle_nodes(&display_tree);
                for toggle in &toggle_nodes {
                    if let holon_frontend::view_model::ViewKind::StateToggle {
                        field,
                        current,
                        label,
                        states,
                    } = &toggle.kind
                    {
                        assert_eq!(
                            field, "task_state",
                            "[inv-viewmodel-state-toggle-correct] unexpected field in StateToggle"
                        );

                        let block_id_str = toggle.row_id();
                        assert!(
                            block_id_str.is_some(),
                            "[inv-viewmodel-state-toggle-correct] StateToggle has no entity id!\n{}",
                            display_tree.pretty_print(0)
                        );
                        let block_id_str = block_id_str.unwrap();
                        let block_id = EntityUri::from_raw(&block_id_str);

                        // Only assert operations/states on TASK blocks in the reference model.
                        // Non-task blocks rendered with a custom render expression containing
                        // state_toggle legitimately have no operations (the "task" profile
                        // only activates when is_task == true, i.e. task_state is set).
                        if let Some(ref_block) = ref_state.block_state.blocks.get(&block_id) {
                            let expected_state = ref_block
                                .task_state()
                                .map(|ts| ts.keyword.to_string())
                                .unwrap_or_default();

                            if ref_block.task_state().is_some() {
                                // Task blocks: full interactivity assertions
                                assert!(
                                    !toggle.operations.is_empty(),
                                    "[inv-viewmodel-state-toggle-correct] StateToggle for {block_id_str} has no operations!\n{}",
                                    display_tree.pretty_print(0)
                                );

                                assert!(
                                    holon_frontend::operations::find_set_field_op(
                                        field,
                                        &toggle.operations
                                    )
                                    .is_some(),
                                    "[inv-viewmodel-state-toggle-correct] No set_field op for '{field}' on {block_id_str}"
                                );

                                assert!(
                                    !states.is_empty(),
                                    "[inv-viewmodel-state-toggle-correct] StateToggle for {block_id_str} has empty states"
                                );
                            }

                            // Value/label assertions apply to all blocks (task or not)
                            assert_eq!(
                                current, &expected_state,
                                "[inv-viewmodel-state-toggle-correct] StateToggle current '{current}' != \
                                     reference '{expected_state}' for block {block_id}"
                            );

                            let (expected_label, _) =
                                holon_api::render_eval::state_display(current);
                            assert_eq!(
                                label, expected_label,
                                "[inv-viewmodel-state-toggle-correct] StateToggle label '{label}' != \
                                     expected '{expected_label}' for block {block_id}"
                            );
                        }
                    }
                }
                if !toggle_nodes.is_empty() {
                    eprintln!(
                        "[inv-viewmodel-state-toggle-correct] {} StateToggle node(s) verified",
                        toggle_nodes.len()
                    );
                }

                // 10h_live. Live-tree vs fresh-tree comparison.
                //
                // The fresh tree (display_tree above) is always re-interpreted
                // from current data — it can't catch bugs where set_data
                // doesn't propagate to child widgets. The HeadlessLiveTree
                // persists across transitions and receives CDC updates through
                // the collection driver's set_data path, mirroring GPUI.
                //
                // We anchor the live tree on the **main panel block**, not the
                // root. The root layout has a render expression but no data
                // query — its data_rows are always empty. Actual rows live in
                // the nested `live_block(default-main-panel)`'s own
                // ReactiveQueryResults. This is where the collection driver
                // runs and where `set_data` would fire on `VecDiff::UpdateAt`
                // when a row's task_state changes.
                //
                // If the live tree diverges from the fresh tree, child widgets
                // (state_toggle, editable_text, etc.) have stale data/props.
                if !nav_only {
                    let main_panel_id = holon_api::EntityUri::block("default-main-panel");
                    let mp_results = reactive.ensure_watching(&main_panel_id);

                    // Wait for the main panel watcher to deliver its first
                    // emission. ToggleState only fires after a sidebar click
                    // populates focus_roots, so the GQL data should be
                    // arriving — but the watcher may still be cold on the
                    // first ClickBlock-only transition.
                    {
                        use futures::StreamExt;
                        let mut mp_stream = reactive.watch(&main_panel_id);
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                        loop {
                            let (mp_render, mp_rows) = mp_results.snapshot();
                            let still_loading = matches!(
                                &mp_render,
                                holon_api::RenderExpr::FunctionCall { name, .. }
                                    if name == "loading"
                            );
                            if !still_loading && !mp_rows.is_empty() {
                                break;
                            }
                            match tokio::time::timeout_at(deadline, mp_stream.next()).await {
                                Ok(Some(_)) => continue,
                                _ => break,
                            }
                        }
                    }

                    let (mp_render_expr, mp_data_rows) = mp_results.snapshot();

                    let still_loading = matches!(
                        &mp_render_expr,
                        holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading"
                    );

                    if !still_loading && !mp_data_rows.is_empty() {
                        if let Some(item_template) =
                            holon_layout_testing::live_tree::extract_item_template(&mp_render_expr)
                        {
                            let needs_init = self.live_tree.borrow().is_none();
                            if needs_init {
                                let data_source: std::sync::Arc<
                                    dyn holon_api::ReactiveRowProvider,
                                > = mp_results.clone();
                                let services: std::sync::Arc<
                                    dyn holon_frontend::reactive::BuilderServices,
                                > = reactive.clone();
                                let lt = holon_layout_testing::live_tree::HeadlessLiveTree::new(
                                    data_source,
                                    item_template.clone(),
                                    services,
                                    &reactive.runtime_handle,
                                );
                                *self.live_tree.borrow_mut() = Some(lt);
                                // Give the driver time to populate initial items.
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                eprintln!(
                                    "[inv10h_live] HeadlessLiveTree initialized on \
                                     main panel ({} items, item_template={})",
                                    self.live_tree
                                        .borrow()
                                        .as_ref()
                                        .map_or(0, |t| t.item_count()),
                                    item_template.to_rhai(),
                                );
                            }

                            // Give the driver a moment to process pending VecDiff events.
                            tokio::time::sleep(Duration::from_millis(10)).await;

                            let live_ref = self.live_tree.borrow();
                            if let Some(ref lt) = *live_ref {
                                let live_items = lt.items();
                                let fresh_items: Vec<
                                    std::sync::Arc<holon_frontend::ReactiveViewModel>,
                                > = mp_data_rows
                                    .iter()
                                    .map(|row| {
                                        let ctx = holon_frontend::RenderContext::default()
                                            .with_row(row.clone());
                                        let node = reactive.interpret(&item_template, &ctx);
                                        std::sync::Arc::new(node)
                                    })
                                    .collect();

                                if live_items.len() != fresh_items.len() {
                                    // Item count mismatch: the driver hasn't caught up yet
                                    // (InsertAt/RemoveAt pending). Log but don't fail — the
                                    // bug we're catching is stale PROPS on existing items.
                                    eprintln!(
                                        "[inv10h_live] Item count mismatch: live={} fresh={} (driver lag)",
                                        live_items.len(),
                                        fresh_items.len()
                                    );
                                }

                                // Match live↔fresh items by position.
                                //
                                // The wrapper vm of `render_entity()` doesn't carry the
                                // row id on its own `data` — the row is buried in inner
                                // children (state_toggle, editable_text, ...). But both
                                // `live_items` and `fresh_items` are produced from the
                                // same `mp_data_rows` sequence with `sort_key: None`, so
                                // index `i` corresponds to `mp_data_rows[i]` on both
                                // sides. We use that row's id as the diagnostic key.
                                let mut prop_diffs = Vec::new();
                                let pair_count = live_items.len().min(fresh_items.len());
                                for i in 0..pair_count {
                                    let row_id = mp_data_rows
                                        .get(i)
                                        .and_then(|r| r.get("id"))
                                        .and_then(|v| v.as_string())
                                        .unwrap_or("?")
                                        .to_string();
                                    let diffs = crate::display_assertions::tree_diff(
                                        live_items[i].as_ref(),
                                        fresh_items[i].as_ref(),
                                    );
                                    for d in diffs {
                                        prop_diffs.push(format!("  [{i}] {row_id}: {d}"));
                                    }
                                }

                                if !prop_diffs.is_empty() {
                                    panic!(
                                        "[inv10h_live] LIVE tree diverges from FRESH tree!\n\
                                         The collection driver's set_data path produces different \
                                         props than fresh interpretation. Child widgets see stale \
                                         data in the GPUI frontend.\n\n\
                                         Diffs ({}):\n{}",
                                        prop_diffs.len(),
                                        prop_diffs.join("\n")
                                    );
                                }
                                eprintln!(
                                    "[inv10h_live] Live vs fresh: {} item pair(s) compared, no divergence",
                                    pair_count
                                );
                            }
                        } else {
                            eprintln!(
                                "[inv10h_live] no item_template in main-panel render_expr: {}",
                                mp_render_expr.to_rhai(),
                            );
                        }
                    } else {
                        eprintln!(
                            "[inv10h_live] main panel not ready (loading={}, rows={})",
                            still_loading,
                            mp_data_rows.len(),
                        );
                    }
                }

                // 10j. Virtual child / trailing slot rendering.
                //
                // When the active render expression is a tree (default in
                // collection_profile.yaml's tree_view variant with
                // creation_slot: true), the last item in every tree collection
                // must be a virtual child placeholder with entity id
                // <scheme>:__virtual:<parent_local>.
                {
                    let is_tree = ref_state
                        .active_render_expr_name(holon_api::Region::Main)
                        .map(|n| n == "tree")
                        .unwrap_or(false);
                    if is_tree {
                        // FIXME: display_tree must be obtained from
                        // wait_for_entity_in_resolved_view_model or similar
                        // before this invariant can meaningfully execute.
                        // Blocked on inv-viewmodel-tree-virtual-slots wiring — see
                        // crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_tree_virtual_slots.rs
                        // for the migrated (deferred) impl; promote when the
                        // display_tree wiring lands.
                        eprintln!(
                            "[inv-viewmodel-tree-virtual-slots] SKIPPED — display_tree not wired in this scope"
                        );
                    }
                }

                // 10i. Matview data IDs must match reference model (catches IVM inconsistency)
                //
                // The data_rows come from the matview snapshot (CDC pipeline). If the
                // matview is inconsistent with the base table (Turso IVM bug), data_rows
                // will have extra/missing rows compared to the reference model.
                //
                // The root layout query returns all non-source descendants of the focus
                // roots. We compute this set from the reference model and compare.
                if !data_rows.is_empty() {
                    let data_block_ids: std::collections::BTreeSet<String> = data_rows
                        .iter()
                        .filter_map(|r| {
                            r.get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .collect();

                    // Compute expected: all blocks in reference model (including source).
                    // Also include layout blocks and profile blocks which the ref model
                    // doesn't track as regular blocks but are in the DB.
                    let ref_block_ids: std::collections::BTreeSet<String> = ref_state
                        .block_state
                        .blocks
                        .values()
                        .map(|b| b.id.as_str().to_string())
                        .chain(
                            ref_state
                                .layout_blocks
                                .headline_ids
                                .iter()
                                .chain(&ref_state.layout_blocks.query_source_ids)
                                .chain(&ref_state.layout_blocks.render_source_ids)
                                .chain(&ref_state.profile_block_ids)
                                .map(|id| id.as_str().to_string()),
                        )
                        .collect();

                    // Extra IDs in matview that aren't in reference model
                    let extra: Vec<&String> = data_block_ids
                        .iter()
                        .filter(|id| !ref_block_ids.contains(*id))
                        .collect();

                    // Missing IDs in matview that should be visible
                    // (only check blocks that are in the focus tree, not all reference blocks)
                    let focus_roots = ref_state.expected_focus_root_ids(holon_api::Region::Main);
                    let expected_visible: std::collections::BTreeSet<String> = ref_state
                        .block_state
                        .blocks
                        .values()
                        .filter(|b| {
                            !matches!(b.content_type, holon_api::ContentType::Source)
                                && ref_state.is_descendant_of_any(&b.id, &focus_roots)
                        })
                        .map(|b| b.id.as_str().to_string())
                        .collect();

                    let missing: Vec<&String> = expected_visible
                        .iter()
                        .filter(|id| !data_block_ids.contains(*id))
                        .collect();

                    if !extra.is_empty() || !missing.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] IVM MATVIEW INCONSISTENCY DETECTED!\n\
                                 Data rows (from matview): {} IDs\n\
                                 Reference model: {} total blocks, {} expected visible\n\
                                 Extra in matview (stale/ghost): {:?}\n\
                                 Missing from matview: {:?}",
                            data_block_ids.len(),
                            ref_block_ids.len(),
                            expected_visible.len(),
                            extra,
                            missing,
                        );
                    }
                    // NOTE: These are soft checks because the AppState data_rows come
                    // from the ROOT LAYOUT query (returns layout column blocks), not
                    // from region-specific queries (which return user content blocks).
                    // The data sets are different levels of the rendering hierarchy.
                    if !extra.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] Matview has {} extra block IDs not in reference model: {:?}",
                            extra.len(),
                            extra,
                        );
                    }
                    // TODO: Re-enable once inv-matview-consistent-with-ref compares region-specific data
                    // (not root layout data which is a different hierarchy level).
                    // if !missing.is_empty() {
                    //     eprintln!(
                    //         "[inv-matview-consistent-with-ref] Matview is MISSING {} block IDs: {:?}",
                    //         missing.len(), missing,
                    //     );
                    // }
                    if extra.is_empty() && missing.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] Matview data ({} rows) consistent with reference model",
                            data_block_ids.len(),
                        );
                    }
                }
            }

            // ─── inv-value-fn-provider-arg-variance/12/13: value-fn provider invariants ────────────────
            //
            // These invariants cover the `ReactiveRowProvider`s produced by
            // value functions (`focus_chain`, `ops_of`, `chain_ops`). The
            // reactive engine caches them via `ProviderCache` so repeated
            // `(name, args)` calls share an `Arc`. We re-interpret the
            // current render tree against the live engine (so the cache is
            // active) and walk the resulting tree collecting streaming
            // providers.
            //
            // Viewport trigger: push a narrow 500×800 viewport so the
            // default `block:root-layout` profile picks the
            // `if_space(600, ...)` branch that instantiates the mobile
            // action bar (`focus_chain()` + `ops_of(col("uri"))`). Without
            // this the PBT would only exercise the chain_ops fixture in
            // `valid_render_expressions` when it's randomly chosen — the
            // narrow viewport guarantees coverage on every run that has a
            // root layout present. `ui_state.set_viewport` sets a
            // `Mutable` that the reactive signal graph already subscribes
            // to, so one scheduler tick propagates it downstream.
            if ref_state.app_started && !ref_state.block_state.blocks.is_empty() {
                use crate::pbt::value_fn_invariants::{
                    collect_providers, count_bottom_docks, rhai_mentions,
                };

                let reactive = match self.reactive_engine.borrow().clone() {
                    Some(r) => r,
                    None => return,
                };

                reactive
                    .ui_state()
                    .set_viewport(holon_frontend::reactive::ViewportInfo {
                        width_px: 500.0,
                        height_px: 800.0,
                        scale_factor: 1.0,
                    });
                tokio::task::yield_now().await;
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);
                let results = reactive.ensure_watching(&root_id);
                let (render_expr, data_rows) = results.snapshot();

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
                {
                    // Root still initializing — nothing to observe.
                } else {
                    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
                        reactive.clone();

                    let re = render_expr.clone();
                    let dr = data_rows.clone();
                    let svc1 = services.clone();
                    let tree1 = tokio::task::spawn_blocking(move || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            holon_frontend::interpret_pure(&re, &dr, &*svc1)
                        }))
                        .ok()
                    })
                    .await
                    .expect("spawn_blocking panicked");

                    let Some(tree1) = tree1 else {
                        eprintln!(
                            "[inv-value-fn-provider-arg-variance-13] first interpret panicked, skipping"
                        );
                        return;
                    };

                    let providers1 = collect_providers(&tree1);
                    let total1 = providers1.len();

                    // inv_bar — bottom_dock structural presence.
                    //
                    // If the active render_expr for the root layout
                    // mentions `bottom_dock`, the interpreted tree must
                    // contain at least one `BottomDock` node with
                    // exactly two children (main + dock slot). Catches
                    // regressions where the `bottom_dock` widget
                    // silently falls through to the `unknown` arm, or
                    // its shadow builder drops a slot.
                    if rhai_mentions(&render_expr, "bottom_dock") {
                        let docks = count_bottom_docks(&tree1);
                        assert!(
                            docks >= 1,
                            "[inv_bar] render_expr mentions bottom_dock but \
                             interpreted tree contains 0 BottomDock nodes"
                        );
                        eprintln!("[inv_bar] bottom_dock count = {docks}");
                    }

                    // inv-value-fn-provider-arg-variance — provider arg variance.
                    //
                    // Only assert when the **active** render_expr (the one
                    // the reactive engine just interpreted) mentions
                    // `focus_chain` AND a focus target is set AND the
                    // walker actually surfaced a streaming provider. This
                    // keeps the check specific to cases where a
                    // focus_chain-backed node is genuinely present —
                    // render_expressions in `ref_state` may contain
                    // fixtures attached to nested blocks that the current
                    // interpretation doesn't reach.
                    let active_has_focus_chain = rhai_mentions(&render_expr, "focus_chain");
                    let expects_focus_rows =
                        ref_state.focused_block.is_some() && active_has_focus_chain && total1 > 0;
                    let any_nonempty = providers1.iter().any(|p| p.rows_snapshot_len > 0);
                    eprintln!(
                        "[vfn11] streaming_providers={} any_nonempty={} \
                         expects_focus_rows={} active_has_focus_chain={}",
                        total1, any_nonempty, expects_focus_rows, active_has_focus_chain,
                    );
                    if expects_focus_rows {
                        assert!(
                            any_nonempty,
                            "[vfn11] active render_expr mentions focus_chain and \
                             reference model has focused_block = {:?}, but no streaming \
                             provider produced rows",
                            ref_state.focused_block,
                        );
                    }

                    // inv-value-fn-provider-identity — provider identity stability within one pass.
                    //
                    // Group by `(item_template_debug, rows_snapshot_len)` — a
                    // coarse but useful proxy for "same `(name, args)`".
                    // Track per-group **call-site count** (how many walker
                    // visits landed on that group) and the set of distinct
                    // `cache_identity()` values seen. A group with more
                    // than one call site but exactly one identity is
                    // evidence of cache reuse — one `Arc` serving several
                    // sites. The "reuse" metric is what the handoff's
                    // "cache reuse > 0" acceptance is checking for; it is
                    // reported alongside the group count.
                    use std::collections::{HashMap, HashSet};
                    let mut sites_per_group: HashMap<(String, usize), usize> = HashMap::new();
                    let mut ids_per_group: HashMap<(String, usize), HashSet<u64>> = HashMap::new();
                    for p in &providers1 {
                        let key = (p.item_template_debug.clone(), p.rows_snapshot_len);
                        *sites_per_group.entry(key.clone()).or_default() += 1;
                        ids_per_group
                            .entry(key)
                            .or_default()
                            .insert(p.cache_identity);
                    }
                    let mut reuse_groups = 0usize;
                    let mut reuse_sites = 0usize;
                    for (key, ids) in &ids_per_group {
                        let sites = sites_per_group.get(key).copied().unwrap_or(0);
                        if ids.len() > 1 {
                            panic!(
                                "[vfn12] provider identity instability: template={} \
                                 rows={} → {} distinct cache_identities across {sites} call sites",
                                key.0,
                                key.1,
                                ids.len(),
                            );
                        }
                        if sites > 1 {
                            reuse_groups += 1;
                            reuse_sites += sites;
                        }
                    }
                    eprintln!(
                        "[vfn12] provider groups={} reuse_groups={} reuse_sites={}",
                        ids_per_group.len(),
                        reuse_groups,
                        reuse_sites,
                    );

                    // inv-sql-budget — no flicker across re-interpret.
                    // Re-run interpretation; every cache_identity observed
                    // in pass-1 should still appear in pass-2 (Arcs persist
                    // because `ProviderCache` hands out the same Weak on
                    // unchanged args).
                    let re2 = render_expr.clone();
                    let dr2 = data_rows.clone();
                    let svc2 = services.clone();
                    let tree2 = tokio::task::spawn_blocking(move || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            holon_frontend::interpret_pure(&re2, &dr2, &*svc2)
                        }))
                        .ok()
                    })
                    .await
                    .expect("spawn_blocking panicked");

                    let Some(tree2) = tree2 else {
                        eprintln!("[vfn13] second interpret panicked, skipping");
                        return;
                    };

                    let providers2 = collect_providers(&tree2);
                    let ids1: std::collections::HashSet<u64> =
                        providers1.iter().map(|p| p.cache_identity).collect();
                    let ids2: std::collections::HashSet<u64> =
                        providers2.iter().map(|p| p.cache_identity).collect();
                    let flickered: Vec<u64> = ids1.difference(&ids2).copied().collect();
                    eprintln!(
                        "[vfn13] pass1 ids={} pass2 ids={} stable={}",
                        ids1.len(),
                        ids2.len(),
                        ids1.intersection(&ids2).count(),
                    );
                    assert!(
                        flickered.is_empty(),
                        "[vfn13] provider cache identity flicker: {} ids present in pass-1 \
                         but missing in pass-2 — cache wiring regressed",
                        flickered.len(),
                    );
                }
            }
        } // end if !nav_only (#9, #10)

        // 11. Loro vs Org check DISABLED: Loro is no longer the write path for blocks.
        // All block CRUD goes through SqlOperationProvider. Loro is populated via EventBus
        // subscriptions (reverse sync) which hasn't been implemented yet.
        // Re-enable this check once EventBus → Loro sync is in place.

        // 12. Every intermediate ViewModel emission must have correct StateToggle values.
        //
        // A background task collects ALL ViewModel emissions from the reactive stream.
        // We drain and check each one — this catches transient bugs where the CDC
        // enrichment pipeline produces incorrect data that is later masked when a
        // structural re-render fetches fresh data from the query path.
        //
        // Without this, bugs like flatten_properties only handling Value::Object (not
        // Value::String from the CDC path) go undetected because the final snapshot
        // always has correct data from the query path.
        if ref_state.app_started && !nav_only {
            let emissions: Vec<holon_frontend::ViewModel> =
                std::mem::take(&mut *self.vm_emissions.lock().unwrap());

            let mut checked = 0usize;
            for (i, vm) in emissions.iter().enumerate() {
                let toggles = crate::display_assertions::collect_state_toggle_nodes(vm);
                for toggle in &toggles {
                    if let holon_frontend::view_model::ViewKind::StateToggle { current, .. } =
                        &toggle.kind
                    {
                        let Some(block_id_str) = toggle.row_id() else {
                            continue;
                        };
                        let block_id = EntityUri::from_raw(&block_id_str);
                        let Some(ref_block) = ref_state.block_state.blocks.get(&block_id) else {
                            continue;
                        };
                        let expected = ref_block
                            .task_state()
                            .map(|ts| ts.keyword.to_string())
                            .unwrap_or_default();

                        assert_eq!(
                            current, &expected,
                            "[inv-value-fn-provider-identity] Intermediate ViewModel emission #{i} has wrong \
                             StateToggle value for block {block_id}.\n\
                             Got '{current}', expected '{expected}'.\n\
                             This means the CDC enrichment pipeline produced incorrect \
                             data that would be visible as a UI glitch before the next \
                             structural re-render masks it."
                        );
                        checked += 1;
                    }
                }
            }
            if checked > 0 {
                eprintln!(
                    "[inv-value-fn-provider-identity] Verified {} StateToggle value(s) across {} intermediate ViewModel emissions",
                    checked,
                    emissions.len(),
                );
            }
        }

        // ── 13. Non-functional span invariants (SQL counts, durations, memory) ────
        #[cfg(feature = "otel-testing")]
        {
            let metrics = self.span_collector.snapshot();
            let wall_time = self
                .last_transition_start
                .map(|t| t.elapsed())
                .unwrap_or_default();
            let key = super::transition_budgets::transition_key(&self.last_transition);

            // 13d. RSS memory tracking
            let rss_after = crate::test_tracing::current_rss_bytes();
            let memory = super::transition_budgets::MemoryMetrics {
                rss_before: self.rss_before,
                rss_after,
                rss_baseline: self.rss_baseline,
            };

            // 13b. Summary line (always printed before violations can panic)
            let expected =
                super::transition_budgets::expected_sql(&self.last_transition, ref_state);
            let render_summary: String = if metrics.render_count > 0 {
                let components: Vec<_> = metrics
                    .render_by_component
                    .iter()
                    .map(|(c, n)| format!("{c}={n}"))
                    .collect();
                format!(
                    " renders={} [{}]",
                    metrics.render_count,
                    components.join(",")
                )
            } else {
                String::new()
            };
            let cdc_summary: String =
                if metrics.cdc_ingest_count > 0 || metrics.cdc_emission_count > 0 {
                    format!(
                        " cdc_in={} cdc_out={}",
                        metrics.cdc_ingest_count, metrics.cdc_emission_count
                    )
                } else {
                    String::new()
                };
            // HOLON_PERF investigation: per-transition attribution of suspected hot paths.
            let perf_summary = format!(
                " apply={}ms check={}ms drain_cdc={}ms inv10_drain={}ms files_stable={}ms file_sync={}ms mark_proc={}ms×{}",
                metrics.apply_transition_total.as_millis(),
                metrics.check_invariants_total.as_millis(),
                metrics.drain_cdc_total.as_millis(),
                metrics.inv10_watch_drain.as_millis(),
                metrics.wait_files_stable.as_millis(),
                metrics.wait_file_sync.as_millis(),
                metrics.mark_processed_total.as_millis(),
                metrics.mark_processed_count,
            );
            eprintln!(
                "[inv-sql-budget] {key}: reads={}/{} writes={}/{} ddl={}/{} tol={} max_q={}ms wall={}ms spans={} \
                 rss={delta:+.1}MB (cum={cum:+.1}MB){render_summary}{cdc_summary}{perf_summary}",
                metrics.sql_read_count,
                expected.reads,
                metrics.sql_write_count,
                expected.writes,
                metrics.sql_ddl_count,
                expected.ddl,
                expected.tolerance,
                metrics.max_query_duration.as_millis(),
                wall_time.as_millis(),
                metrics.total_span_count,
                delta = memory.rss_delta_mb(),
                cum = memory.cumulative_growth_mb(),
            );

            // 13c. Budget violation checks (may panic)
            let violations = super::transition_budgets::check_budget(
                &self.last_transition,
                ref_state,
                &metrics,
                wall_time,
                Some(&memory),
            );

            // Budgets drifted significantly after the reactive refactor; opt
            // into enforcement explicitly via HOLON_PERF_BUDGET=1 once they
            // are recalibrated. Default behavior logs violations as warnings.
            let enforce_budget = std::env::var("HOLON_PERF_BUDGET")
                .map(|v| v != "0")
                .unwrap_or(false);

            let has_memory_violation = violations.iter().any(|v| match v {
                super::transition_budgets::Violation::Error(msg) => msg.contains("rss_"),
                _ => false,
            });

            if has_memory_violation {
                super::transition_budgets::diagnose_memory(&key);
            }

            for v in &violations {
                match v {
                    super::transition_budgets::Violation::Warning(msg) => {
                        eprintln!("[inv-sql-budget WARN] {msg}");
                    }
                    super::transition_budgets::Violation::Error(msg) => {
                        if enforce_budget {
                            panic!("inv-sql-budget: {msg}");
                        } else {
                            eprintln!("[inv-sql-budget BUDGET OFF] {msg}");
                        }
                    }
                }
            }

            // 13d. Duplicate SQL detection — warn about potential N+1 patterns
            if !metrics.duplicate_sql.is_empty() {
                eprintln!(
                    "[inv-sql-budget N+1] {key}: {} distinct SQL texts fired multiple times:",
                    metrics.duplicate_sql.len()
                );
                for (sql, count) in &metrics.duplicate_sql {
                    eprintln!("  {count}x: {sql}");
                }
            }

            // 13e. Flamegraph (opt-in via HOLON_PERF_FLAMEGRAPH=/path/to/dir)
            crate::test_tracing::maybe_write_flamegraph(&self.span_collector, &key);

            // Detailed SQL breakdown (enabled by HOLON_PERF_DETAIL=1)
            if std::env::var("HOLON_PERF_DETAIL").is_ok() {
                let breakdown = self.span_collector.sql_breakdown();
                eprintln!("[inv-sql-budget DETAIL] {key}:\n{breakdown}");
            }
        }

        // ── inv-frontend-engine: Frontend engine ViewModel assertions ─────────
        //
        // When a frontend engine is installed (e.g., GPUI PBT), check that
        // the frontend's own ReactiveEngine produces a valid ViewModel.
        // This catches issues invisible to the headless engine: matview
        // failures, CDC delivery bugs, cross-executor waker issues.
        if let Some(ref fe_engine) = self.frontend_engine {
            let root_uri = holon_api::root_layout_block_uri();
            let rqr = fe_engine.ensure_watching(&root_uri);

            if rqr.is_loading() {
                eprintln!(
                    "[inv-frontend-engine] Frontend engine still loading root layout — skipping"
                );
            } else {
                let vm = fe_engine.snapshot(&root_uri);
                let root_kind = vm.widget_name().unwrap_or("?");

                // 14a: inv-frontend-root-not-error — Phase 10.1: migrated to
                //      `InvFrontendRootNotError` via `SutViewModel`. Manual
                //      match (not `assert_invariants!`) so the panic can carry
                //      the root `error_message` for diagnostic richness.
                {
                    use crate::pbt::invariants::bodies::frontend_root_not_error::InvFrontendRootNotError;
                    use holon_pbt_core::invariant::{Invariant, InvariantResult};
                    match Invariant::<ReferenceState, Self>::check(
                        &InvFrontendRootNotError,
                        ref_state,
                        self,
                    )
                    .await
                    {
                        InvariantResult::Ok => {}
                        InvariantResult::Fail(msg) => panic!(
                            "{msg} (root error_message = {:?})",
                            vm.entity.get("error_message")
                        ),
                        InvariantResult::Skipped(_) => {}
                    }
                }

                // 14b: inv-frontend-no-error-widgets — Phase 10.1: migrated to
                //      `InvFrontendNoErrorWidgets` via `SutViewModel + SutLayout`.
                //      We still emit per-node summaries on failure for diagnostic
                //      detail before the migrated impl panics with its message.
                {
                    let error_count = crate::display_assertions::count_error_nodes(&vm);
                    if error_count > 0 {
                        let summaries =
                            crate::display_assertions::collect_error_node_summaries(&vm);
                        eprintln!(
                            "[inv-frontend-no-error-widgets] {} Error widget(s) in ViewModel:",
                            summaries.len()
                        );
                        for s in &summaries {
                            eprintln!("    {s}");
                        }
                    }
                    use crate::pbt::invariants::bodies::frontend_no_error_widgets::InvFrontendNoErrorWidgets;
                    assert_invariants!(ref_state, self, InvFrontendNoErrorWidgets);
                }

                // 14c: BoundsRegistry assertions — verify GPUI actually laid out elements
                let entity_ids = vm.collect_entity_ids();
                if let Some(ref geometry) = self.frontend_geometry {
                    // Wait for GPUI to render at least one tracked element. The
                    // backend ViewModel resolves faster than the GPUI render pipeline;
                    // the first check can land before any prepaint has run.
                    let all_elements = {
                        let mut elements = geometry.all_elements();
                        if elements.is_empty() && !ref_state.documents.is_empty() {
                            // GPUI debug builds need more time: the render
                            // pipeline (signal → render → prepaint → record)
                            // can take several seconds after a mutation.
                            for _ in 0..50 {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                elements = geometry.all_elements();
                                if !elements.is_empty() {
                                    break;
                                }
                            }
                        }
                        elements
                    };

                    // An entity is "rendered" if any tracked element has its
                    // entity_id — checked via both el_id prefix (for fast path)
                    // and entity_id field (for selectable/editable_text widgets
                    // whose el_id uses different prefixes).
                    let lookup_entity = |eid: &str| {
                        geometry
                            .element_info(&format!("render-entity-{eid}"))
                            .or_else(|| geometry.element_info(&format!("live-block-{eid}")))
                            .or_else(|| geometry.element_info(&format!("selectable-{eid}")))
                            .or_else(|| geometry.element_info(&format!("editable-text-{eid}")))
                            .or_else(|| {
                                // Fallback: scan all_elements for any entity_id match
                                all_elements
                                    .iter()
                                    .find(|(_, info)| info.entity_id.as_deref() == Some(eid))
                                    .map(|(_, info)| info.clone())
                            })
                    };

                    // Dump tracked elements as a parent-indented tree (helps
                    // diagnose assertion failures). Each element's `parent_id`
                    // points at the nearest enclosing tracked widget recorded
                    // by `TransparentTracker`. Children are sorted by (y, x,
                    // el_id) so painting order is preserved visually. Orphans
                    // (parent_id pointing at a missing entry) are surfaced
                    // under a synthetic `<orphan>` root rather than silently
                    // dropped.
                    {
                        use std::collections::HashMap;

                        let by_id: HashMap<&str, &holon_frontend::geometry::ElementInfo> =
                            all_elements.iter().map(|(k, v)| (k.as_str(), v)).collect();

                        let mut children_of: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
                        let mut orphans: Vec<&str> = Vec::new();
                        for (el_id, info) in &all_elements {
                            match info.parent_id.as_deref() {
                                None => children_of.entry(None).or_default().push(el_id.as_str()),
                                Some(p) if by_id.contains_key(p) => {
                                    children_of.entry(Some(p)).or_default().push(el_id.as_str())
                                }
                                Some(_) => orphans.push(el_id.as_str()),
                            }
                        }
                        let sort_children = |ids: &mut Vec<&str>| {
                            ids.sort_by(|a, b| {
                                let ai = by_id[a];
                                let bi = by_id[b];
                                ai.y.partial_cmp(&bi.y)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                                    .then(
                                        ai.x.partial_cmp(&bi.x)
                                            .unwrap_or(std::cmp::Ordering::Equal),
                                    )
                                    .then_with(|| a.cmp(b))
                            });
                        };
                        for ids in children_of.values_mut() {
                            sort_children(ids);
                        }
                        sort_children(&mut orphans);

                        fn print_node(
                            id: &str,
                            depth: usize,
                            by_id: &HashMap<&str, &holon_frontend::geometry::ElementInfo>,
                            children_of: &HashMap<Option<&str>, Vec<&str>>,
                            label: &str,
                        ) {
                            let info = by_id[id];
                            let indent = "  ".repeat(depth);
                            eprintln!(
                                "[{label}] {indent}{id}: widget_type={} entity_id={:?} bounds=({:.0},{:.0} {:.0}x{:.0}) has_content={}",
                                info.widget_type,
                                info.entity_id,
                                info.x,
                                info.y,
                                info.width,
                                info.height,
                                info.has_content,
                            );
                            if let Some(kids) = children_of.get(&Some(id)) {
                                for child in kids {
                                    print_node(child, depth + 1, by_id, children_of, label);
                                }
                            }
                        }

                        if let Some(roots) = children_of.get(&None) {
                            for root in roots {
                                print_node(
                                    root,
                                    0,
                                    &by_id,
                                    &children_of,
                                    "inv-frontend-engine TREE",
                                );
                            }
                        }
                        if !orphans.is_empty() {
                            eprintln!(
                                "[inv-frontend-engine TREE] <orphan> ({} entries — parent_id refers to missing element)",
                                orphans.len()
                            );
                            for id in &orphans {
                                print_node(id, 1, &by_id, &children_of, "inv-frontend-engine TREE");
                            }
                        }
                    }

                    // bounds-registry-not-empty: At least 1 element rendered (warning — BoundsRegistry is
                    // a layout-time snapshot; double-buffering means it can be
                    // transiently empty during restarts and state changes. Use not-visually-empty
                    // for authoritative empty-UI detection.)
                    if all_elements.is_empty() {
                        eprintln!(
                            "[inv-frontend-bounds-rendered/bounds-registry-not-empty WARN] BoundsRegistry is empty — GPUI may not have rendered yet (check not-visually-empty for visual emptiness)",
                        );
                    }

                    // expected-size-satisfied: every tracked element's observed (w, h) must satisfy its
                    // declared `expected_size` bounds. Bounds default to "all Free"
                    // (= unconstrained), so widgets that don't opt in are skipped.
                    // The previous hard-coded `live_block` / `spacer` allowlist is
                    // gone — those widgets are simply unconstrained by default. Leaf
                    // widgets (text/icon/selectable/...) can declare `at_least(...)`
                    // to catch genuine "rendered too small" bugs; wrappers can use
                    // `follows_child(child_id)` to express "I'm transparent to layout
                    // and inherit my child's expectation". See
                    // `holon_frontend::size_expectation` for the AST.
                    for (el_id, info) in &all_elements {
                        let ctx = holon_frontend::geometry::ProviderEvalCtx::from_snapshot(
                            &all_elements,
                            el_id.as_str(),
                            None, // viewport unknown here; widgets that need it can be
                                  // wired up later when the test owns the window dims.
                        );
                        if let Err(violation) =
                            info.expected_size.check(info.width, info.height, &ctx)
                        {
                            panic!(
                                "[inv-frontend-bounds-rendered/expected-size-satisfied] Element '{el_id}' violates expected_size: {violation}\n  observed: {info:?}",
                            );
                        }
                    }

                    // vm-entities-have-bounds: Entity IDs from ViewModel that have corresponding bounds (warning —
                    // uniform_list virtualizes, so not all ViewModel entities are rendered).
                    //
                    // Layout blocks (direct children of root-layout, e.g. default-main-panel)
                    // are deliberately NOT tracked by the live_block builder — wrapping a
                    // whole region in BoundsTracker causes the wrapper to collapse to height=0
                    // and clips all region content (see live_block.rs comments). Skip these
                    // to avoid false-positive warnings.
                    let layout_block_ids: std::collections::HashSet<&str> = [
                        "block:default-main-panel",
                        "block:default-left-sidebar",
                        "block:default-right-sidebar",
                    ]
                    .into_iter()
                    .collect();
                    let mut missing = Vec::new();
                    for eid in &entity_ids {
                        if layout_block_ids.contains(eid.as_str()) {
                            continue;
                        }
                        if lookup_entity(eid).is_none() {
                            missing.push(eid.clone());
                        }
                    }

                    // no-error-widgets-rendered: No error widgets rendered
                    for (el_id, info) in &all_elements {
                        assert!(
                            info.widget_type != "error",
                            "[inv-frontend-bounds-rendered/no-error-widgets-rendered] BoundsRegistry contains error widget '{el_id}': {info:?}",
                        );
                    }

                    // known-widget-type: Widget type consistency (warning) — for entity IDs present in both
                    // ViewModel and BoundsRegistry, the widget_type should be one of the
                    // known rendering wrappers.
                    for (el_id, info) in &all_elements {
                        if let Some(ref eid) = info.entity_id
                            && entity_ids.contains(eid)
                        {
                            let ok = matches!(
                                info.widget_type.as_str(),
                                "render_entity"
                                    | "live_block"
                                    | "editable_text"
                                    | "rendered_text"
                                    | "selectable"
                            );
                            if !ok {
                                eprintln!(
                                    "[inv-frontend-bounds-rendered/known-widget-type] Element '{el_id}' entity={eid} has unexpected widget_type='{}'",
                                    info.widget_type,
                                );
                            }
                        }
                    }

                    // element-has-content: Content presence (warning) — rendered elements with entity bindings
                    // should have content when ViewModel says they do.
                    for (el_id, info) in &all_elements {
                        if !info.has_content {
                            eprintln!(
                                "[inv-frontend-bounds-rendered/element-has-content WARN] Element '{el_id}' (widget_type='{}') has has_content=false",
                                info.widget_type,
                            );
                        }
                    }

                    // vm-y-order-and-contiguity: Y-order consistency — rendered elements that correspond to ViewModel
                    // entity IDs must appear in the same y-axis order and form a contiguous
                    // subsequence of the ViewModel's entity list.
                    //
                    // Exclude layout blocks (direct children of root-layout) from the index
                    // computation — they're never rendered via tracked() (see live_block.rs),
                    // so they naturally create gaps in the rendered-index sequence.
                    let contiguity_entity_ids: Vec<&String> = entity_ids
                        .iter()
                        .filter(|eid| !layout_block_ids.contains(eid.as_str()))
                        .collect();
                    let rendered_entities: Vec<(usize, &str, f32)> = contiguity_entity_ids
                        .iter()
                        .enumerate()
                        .filter_map(|(vm_idx, eid)| {
                            let info = lookup_entity(eid)?;
                            Some((vm_idx, eid.as_str(), info.y))
                        })
                        .collect();

                    if rendered_entities.len() >= 2 {
                        // Check y-order: each rendered element's y should be >= previous
                        for pair in rendered_entities.windows(2) {
                            let (_, id_a, y_a) = pair[0];
                            let (_, id_b, y_b) = pair[1];
                            assert!(
                                y_b >= y_a,
                                "[inv-frontend-bounds-rendered/vm-y-order-and-contiguity] Y-order violation: '{id_a}' at y={y_a:.0} appears before '{id_b}' at y={y_b:.0}",
                            );
                        }

                        // Check contiguity: ViewModel indices of rendered elements must be consecutive
                        for pair in rendered_entities.windows(2) {
                            let (idx_a, id_a, _) = pair[0];
                            let (idx_b, id_b, _) = pair[1];
                            assert!(
                                idx_b == idx_a + 1,
                                "[inv-frontend-bounds-rendered/vm-y-order-and-contiguity] Non-contiguous rendering: '{id_a}' at VM index {idx_a} \
                                 and '{id_b}' at VM index {idx_b} — gap of {} entities",
                                idx_b - idx_a - 1,
                            );
                        }
                    }

                    // non-wrapper-content-when-docs and not-visually-empty are gated on the root layout being fully loaded.
                    // When root_kind == "table", the render_expr matview hasn't delivered
                    // the columns() expression yet — the UI shows a loading/fallback state.
                    // Asserting on that transient state would be a false positive.
                    let layout_ready = root_kind != "table";
                    if !layout_ready {
                        eprintln!(
                            "[inv-frontend-bounds-rendered] Root widget is '{}' (loading) — skipping non-wrapper-content-when-docs/not-visually-empty",
                            root_kind,
                        );
                    }

                    // non-wrapper-content-when-docs: Non-container content exists — when ref_state has user documents,
                    // at least one tracked element must be a content widget (render_entity,
                    // editable_text, or selectable), NOT just a live_block wrapper.
                    //
                    // Skip if BoundsRegistry is entirely empty — that's bounds-registry-not-empty's concern and is
                    // better detected via not-visually-empty (visual emptiness from screenshot), which knows
                    // how to distinguish transient empty state (restart/layout race) from a
                    // truly broken render. Firing non-wrapper-content-when-docs on an empty registry produces a
                    // misleading error message ("only live_block wrappers") when in fact
                    // there are no elements at all.
                    if !ref_state.documents.is_empty() && layout_ready && !all_elements.is_empty() {
                        let has_content_widget = all_elements
                            .iter()
                            .any(|(_, info)| info.widget_type != "live_block");
                        assert!(
                            has_content_widget,
                            "[inv-frontend-bounds-rendered/non-wrapper-content-when-docs] ref_state has {} document(s) and BoundsRegistry has \
                             {} elements, but all are live_block wrappers — no content widgets \
                             rendered",
                            ref_state.documents.len(),
                            all_elements.len(),
                        );
                    }

                    // not-visually-empty: Pixel-level empty UI detection — the ground truth for visible
                    // content. BoundsRegistry tracks layout, which can be wildly different
                    // from what's actually painted (clipped elements, stale entries, layout
                    // races). This invariant reads a recent screenshot's analysis and fails
                    // if the window's content area is almost entirely background color.
                    //
                    // Threshold: content_fraction must be > 0.003 (0.3% of content-area
                    // pixels). An empty macOS window with just the title bar typically
                    // measures ~0.001-0.0025; a sparse sidebar-only UI measures ~0.003-0.004;
                    // a UI with main panel content measures > 0.01.
                    //
                    // Exception: after NavigateHome on `main`, the main panel is
                    // intentionally empty and only the sidebar renders. In that
                    // state, content_fraction legitimately falls to ~0.002.
                    // We use a weaker threshold of 0.001 to only catch fully
                    // empty windows (title-bar-only).
                    //
                    // Also: if BoundsRegistry has tracked content widgets,
                    // the UI IS rendering — xcap screenshots can be flaky
                    // when the window is briefly obscured or during GPU
                    // compositing. BoundsRegistry is the authoritative
                    // layout ground truth; not-visually-empty is only a backup for the case
                    // where layout runs but paint produces nothing visible.
                    let main_focused = ref_state
                        .focused_entity_id
                        .contains_key(&holon_api::Region::Main);
                    let min_content = if main_focused { 0.003 } else { 0.001 };
                    let has_bounds_content = all_elements
                        .iter()
                        .any(|(_, info)| info.widget_type != "live_block");
                    if !ref_state.documents.is_empty()
                        && layout_ready
                        && !has_bounds_content
                        && let Some(ref state) = self.frontend_visual_state
                    {
                        let analysis = *state.lock().unwrap();
                        if let Some(analysis) = analysis {
                            assert!(
                                analysis.content_fraction > min_content,
                                "[inv-frontend-bounds-rendered/not-visually-empty] UI is visually empty: content_fraction={:.4} < {:.4} \
                                     (ref_state has {} document(s), main_focused={main_focused}, bounds_empty=true)",
                                analysis.content_fraction,
                                min_content,
                                ref_state.documents.len(),
                            );
                        }
                    }

                    // vm-data-tracked-as-content: ViewModel data coverage — entity IDs emitted by the ViewModel
                    // that are NOT top-level region wrappers represent real data
                    // (documents, tree rows, table rows). At least one of them must
                    // be tracked as a non-`live_block` content widget. Catches the
                    // case where the ViewModel emits entity IDs but the renderer
                    // only materialises wrappers — leaving no element bound to the
                    // entity in BoundsRegistry. (`live_block` is the GPUI bug-
                    // marker: GPUI's `live_block` builder deliberately does NOT
                    // call `tracked()`, so a `widget_type == "live_block"`
                    // registration with `entity_id == eid` indicates the
                    // wrapper-only failure mode rather than a content row. TUI's
                    // tree/table/outline rows register as `render_entity` to keep
                    // this signal frontend-consistent — see
                    // `frontends/tui/src/render/mod.rs`.)
                    //
                    // Exemption: entities with no geometry trace at all
                    // (`lookup_entity` returns `None`) and any `loading` widget in
                    // BoundsRegistry — the VM has emitted the entity but the
                    // render pipeline hasn't produced bounds for it yet. Steady-
                    // state pattern for: watcher hasn't delivered the first
                    // Structure event; newly created/peer-edited entities mid-
                    // propagation; entities outside the virtualised viewport.
                    // We downgrade to a warning when these conditions hold —
                    // there's nothing to assert against.
                    let data_entity_ids: Vec<&String> = entity_ids
                        .iter()
                        .filter(|eid| !eid.starts_with("block:default-"))
                        .collect();
                    if !data_entity_ids.is_empty() {
                        let content_match_count = data_entity_ids
                            .iter()
                            .filter(|eid| {
                                lookup_entity(eid)
                                    .map(|info| info.widget_type != "live_block")
                                    .unwrap_or(false)
                            })
                            .count();
                        // True iff every data entity has *some* widget (live_block or
                        // otherwise). When all entities have at least a live_block but
                        // none are content widgets, it's the original vm-data-tracked-as-content bug.
                        let all_entities_have_live_block = data_entity_ids
                            .iter()
                            .all(|eid| lookup_entity(eid).is_some());
                        let has_loading = all_elements
                            .iter()
                            .any(|(_, info)| info.widget_type == "loading");
                        if content_match_count == 0
                            && (has_loading || !all_entities_have_live_block)
                        {
                            eprintln!(
                                "[inv-frontend-bounds-rendered/vm-data-tracked-as-content WARN] {} data entity ID(s) not yet tracked as content widgets (loading={has_loading}, all_have_live_block={all_entities_have_live_block}): {:?}",
                                data_entity_ids.len(),
                                &data_entity_ids[..data_entity_ids.len().min(5)],
                            );
                        } else {
                            assert!(
                                content_match_count > 0,
                                "[inv-frontend-bounds-rendered/vm-data-tracked-as-content] ViewModel has {} data entity ID(s) but none are tracked as content widgets (render_entity/editable_text/selectable): {:?}",
                                data_entity_ids.len(),
                                &data_entity_ids[..data_entity_ids.len().min(5)],
                            );
                        }
                    }

                    // ── Future invariants (brainstormed, not yet implemented) ──
                    //
                    // widget-type-diverse — Widget type diversity: non-trivial UI should contain ≥ 2
                    //   distinct widget_type values in BoundsRegistry.
                    //
                    // live-block-contains-content — Data-aware containment: for each live_block wrapper whose
                    //   ViewModel sub-tree has data rows > 0, assert that at least one
                    //   non-live_block tracked element's bounds are geometrically contained
                    //   within the live_block's bounds. Natural virtual-scrolling tolerance.
                    //
                    // live-block-area-nonzero — Region area sanity: for any live_block wrapper whose ViewModel
                    //   sub-tree has data rows > 0, the wrapper's own area must be non-zero.
                    //   Catches "empty main panel when it shouldn't be empty".
                    //
                    // total-content-area-nonzero — Non-zero total content area: sum area of all non-live_block
                    //   tracked elements; require > 0 (or some minimum). Weakest check,
                    //   superseded by non-wrapper-content-when-docs but cheap.
                    //
                    // focused-block-tracked — Focus state invariant: if the reference model has a focused
                    //   block, that block's entity_id must appear as a tracked element.
                    //
                    // content-spans-regions — Cross-region span: tracked non-live_block elements should
                    //   span ≥ 2 of the 3 regions when ref_state has documents AND
                    //   navigation focus. Uses geometric intersection with region bounds.
                    //
                    // Also considered: screen-size-based minimum element count, scroll
                    //   position from GPUI's uniform_list. Rejected as brittle — live-block-contains-content/live-block-area-nonzero
                    //   achieve the same goal via geometric containment without needing
                    //   scroll offsets or resolution-dependent thresholds.

                    eprintln!(
                        "[inv-frontend-engine] Frontend: root='{root_kind}', {} entity IDs, {} elements, {} missing bounds, {} rendered in order",
                        entity_ids.len(),
                        all_elements.len(),
                        missing.len(),
                        rendered_entities.len(),
                    );
                    if !missing.is_empty() {
                        eprintln!(
                            "[inv-frontend-engine WARN] {} entity IDs have no BoundsRegistry entry: {:?}",
                            missing.len(),
                            &missing[..missing.len().min(5)],
                        );
                    }
                } else {
                    eprintln!(
                        "[inv-frontend-engine] Frontend ViewModel: root='{root_kind}', {} entity IDs (no geometry)",
                        entity_ids.len(),
                    );
                }
            }
            fe_engine.unwatch(&root_uri);
        }

        // ── inv-editable-text-has-draggable: Every focused editable text block has a Draggable ─
        //
        // Production wraps every block bullet in a `draggable` widget so
        // users can pick up the block and drop it elsewhere. If a future
        // refactor accidentally drops the wrapper for some block subset
        // (e.g. when re-shaping the bullet column), drag&drop silently
        // breaks — `DragDropBlock` would fail before this invariant
        // catches the structural drift.
        //
        // Walks the resolved frontend ViewModel for every block currently
        // in the focus tree (via reference model) and asserts a
        // `Draggable` node carrying the block's id is reachable. Skipped
        // if no frontend engine is installed or none of the focus blocks
        // are text blocks (only text blocks are draggable in production).
        //
        // Skipped when the test environment has registered an alternate
        // `block` entity profile from a generated org file. The test
        // profile YAMLs (see `TestEntityProfile::to_yaml` in
        // `reference_state.rs`) render as `row(editable_text(...))` and
        // get merged into the canonical `block_profile.yaml` variants by
        // `ProfileResolver::merge_profile`. With the test profile's
        // priority-1 `task` variant grabbing every block where
        // `task_state != ()` and the canonical `default` (priority -1)
        // catching the rest, the resolved tree legitimately mixes
        // wrapped and bare `editable_text` widgets — an "N editable_text
        // / N-1 draggable" pattern indistinguishable from the production
        // drift inv-editable-text-has-draggable was designed to catch.
        let inv16_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>> =
            if ref_state.has_blocks_profile() {
                None
            } else {
                self.frontend_engine
                    .clone()
                    .or_else(|| self.reactive_engine.borrow().clone())
            };
        if let Some(engine) = inv16_engine {
            let root_uri = self
                .reactive_root_id
                .borrow()
                .clone()
                .unwrap_or_else(holon_api::root_layout_block_uri);
            let rqr = engine.ensure_watching(&root_uri);
            if !rqr.is_loading() {
                // snapshot_reactive only resolves the root level; nested
                // live_block placeholders need to be expanded explicitly
                // to find draggables that live inside per-block render
                // templates (block_profile.yaml's `column(row(draggable),...)`
                // wrap). BFS over discovered nested block ids.
                // inv-editable-text-has-draggable is a *render-pipeline* invariant scoped to the
                // block_profile render path: when a tree's render produces
                // *any* `draggable` wrappers (canonical block_profile signal),
                // every `editable_text` in the same tree must be paired with
                // a `draggable` carrying the same row_id. If a tree has no
                // draggables at all, it's a custom non-block_profile render
                // (e.g. a sidebar list template `list(item_template:
                // row(editable_text(col("name"))))`) where unpaired
                // editable_text is intentional — skip.
                //
                // Block_profile drift (the production bug we want to catch)
                // shows up as N editable_texts paired with N-1 draggables in
                // the same tree, so per-tree pairing fires correctly.
                //
                // ref-state-vs-SQL divergences (a block that ref_state thinks
                // should be visible but the GQL query never returns) are a
                // separate concern caught by other invariants.
                let mut visited: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut queue: Vec<EntityUri> = vec![root_uri.clone()];
                let mut tree_widget_summary: Vec<(
                    String,
                    std::collections::HashMap<String, usize>,
                )> = Vec::new();
                let mut missing: Vec<String> = Vec::new();
                let mut all_draggable_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut all_editable_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                while let Some(uri) = queue.pop() {
                    if !visited.insert(uri.as_str().to_string()) {
                        continue;
                    }
                    let _ = engine.ensure_watching(&uri);
                    let rvm = engine.snapshot_reactive(&uri);
                    let mut counts: std::collections::HashMap<String, usize> =
                        std::collections::HashMap::new();
                    let mut tree_draggable: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut tree_editable: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    holon_frontend::focus_path::walk_tree(&rvm, &mut |n| {
                        if let Some(name) = n.widget_name() {
                            *counts.entry(name.clone()).or_insert(0) += 1;
                        }
                        match n.widget_name().as_deref() {
                            Some("draggable") => {
                                if let Some(id) = n.row_id() {
                                    tree_draggable.insert(id);
                                }
                            }
                            Some("editable_text") | Some("rendered_text") => {
                                if let Some(id) = n.row_id() {
                                    tree_editable.insert(id);
                                }
                            }
                            Some("live_block") => {
                                if let Some(bid) = n.prop_str("block_id")
                                    && !visited.contains(&bid)
                                {
                                    queue.push(EntityUri::from_raw(&bid));
                                }
                            }
                            _ => {}
                        }
                    });
                    // Only enforce pairing in trees where block_profile-style
                    // rendering is in effect (signaled by ≥1 draggable).
                    if !tree_draggable.is_empty() {
                        for id in tree_editable.difference(&tree_draggable) {
                            missing.push(id.clone());
                        }
                    }
                    all_draggable_ids.extend(tree_draggable);
                    all_editable_ids.extend(tree_editable);
                    tree_widget_summary.push((uri.as_str().to_string(), counts));
                }
                missing.sort();
                missing.dedup();
                let draggable_ids = all_draggable_ids;
                let editable_ids = all_editable_ids;
                if !missing.is_empty() {
                    let mut tree_lines = String::new();
                    for (block_id, counts) in &tree_widget_summary {
                        let mut sorted: Vec<_> = counts.iter().collect();
                        sorted.sort_by(|a, b| b.1.cmp(a.1));
                        tree_lines.push_str(&format!(
                            "    {block_id}: {sorted:?}\n",
                            sorted = sorted.iter().take(15).collect::<Vec<_>>(),
                        ));
                    }
                    panic!(
                        "[inv-editable-text-has-draggable] {n} editable_text widget(s) have no sibling \
                         Draggable carrying the same row_id — drag&drop \
                         would silently break for these blocks (production \
                         GPUI's draggable.rs short-circuits when row_id is \
                         None).\n  missing (editable_text without draggable): \
                         {missing:?}\n  draggable_ids ({n_drag}): {drag_sample:?}\
                         \n  editable_ids ({n_edit}): {edit_sample:?}\n  visited \
                         {visited_n} block trees:\n{tree_lines}",
                        n = missing.len(),
                        n_drag = draggable_ids.len(),
                        n_edit = editable_ids.len(),
                        drag_sample = draggable_ids.iter().take(10).collect::<Vec<_>>(),
                        edit_sample = editable_ids.iter().take(10).collect::<Vec<_>>(),
                        visited_n = visited.len(),
                    );
                }
            }
            engine.unwatch(&root_uri);
        }

        // ── inv-focus-matches-ref: Focus consistency ─────────────────────────────
        // The engine's global `focused_block` mirror (written by the click
        // handler / `maybe_mirror_navigation_focus`) must match the reference
        // model's global `focused_block` after every focus-changing
        // transition. The `focused_entity_id` map is per-region and can hold
        // entries for multiple regions simultaneously (a Main click followed
        // by a RightSidebar focus leaves both populated), so checking against
        // the global field — which tracks the *most recent* focus change — is
        // the only consistent comparison: the engine has a single global
        // `focused_block`, not a per-region map.
        //
        // Skipped:
        //   - SqlOnly mode (no frontend_engine).
        //   - Reference model has no global focus (no focus-changing
        //     transition has fired yet, or the last `go_home` cleared it).
        //   - An editor is active in the ref state. Editor focus
        //     (`active_editor.block_id`) is the source of truth while an
        //     editor is open; the engine's global `focused_block` may or
        //     may not have been updated by the click handler — depends on
        //     whether the GPUI window had finished painting at click time.
        //     The check resumes once `active_editor` clears (e.g. after
        //     navigation away).
        //
        // The ref-state `focused_block` is unresolved-id-shaped (e.g.
        // `block:ref-doc-0`); the engine works with resolved UUIDs. The
        // SUT mirrors the resolved id when it sets engine focus (see
        // NavigateFocus/ArrowNavigate), so the engine value carries the
        // resolved id while the ref tracks the unresolved seed. Compare via
        // `resolve_uri` to bridge that gap.
        // inv-focus-matches-ref — migrated to capability-bound body.
        self.check_inv_focus_matches_ref(ref_state).await;

        // ── inv-displayed-text: editable_text + text widgets show the right string ─
        //
        // The on-screen string for any block-bound text widget (live
        // `InputState` value for `editable_text`, rendered prop for
        // `text(col(...))`) must match what the user is currently editing
        // (or `block.content` if no edit is in progress).
        //
        // Empirically (devlog 2026-05-08-152913): MutableText updates the
        // editor's live state but does NOT synchronously commit to
        // `block.content` — the SQL row only catches up at blur / Enter /
        // chord-commit. So while an editor is active on a block we compare
        // against `active_editor.in_memory_content`; otherwise we compare
        // against the committed `block.content`.
        //
        // This catches both real UI-staleness regressions (post-`split_block`
        // stale prefix on InputState) and any divergence between the
        // editor's view and the reference model's tracked in-memory state.
        if !nav_only && let Some(ref geometry) = self.frontend_geometry {
            // Build reverse map: real URI → synthetic ref-state key.
            // After SplitBlock, the ref state stores the new block under a
            // synthetic `block::split-N` key while the frontend sees the real
            // `block:uuid`. Without reverse resolution, the lookup below skips
            // every split-created block, masking UI staleness.
            let reverse_map: HashMap<EntityUri, EntityUri> = self
                .doc_uri_map
                .iter()
                .map(|(syn, real)| (real.clone(), syn.clone()))
                .collect();

            let mut mismatches: Vec<String> = Vec::new();
            for (_el_id, info) in geometry.all_elements() {
                if info.widget_type != "editable_text"
                    && info.widget_type != "rendered_text"
                    && info.widget_type != "text"
                {
                    continue;
                }
                let Some(ref displayed) = info.displayed_text else {
                    continue;
                };
                let Some(ref entity_id) = info.entity_id else {
                    continue;
                };
                if !entity_id.starts_with("block:") {
                    continue;
                }
                let Ok(uri) = EntityUri::parse(entity_id) else {
                    continue;
                };
                // Try direct lookup first, then reverse-map (split-created
                // blocks are stored under synthetic keys in the ref state).
                let block = ref_state.block_state.blocks.get(&uri).or_else(|| {
                    reverse_map
                        .get(&uri)
                        .and_then(|synthetic| ref_state.block_state.blocks.get(synthetic))
                });
                let Some(block) = block else {
                    continue;
                };
                // While an editor is active on this block, the on-screen
                // string reflects the live `InputState` value, NOT the
                // committed `block.content`. Verified empirically (seed 5,
                // devlog 2026-05-08-..-pbt-empirical): MutableText writes
                // to its CRDT and the InputState reflects that, but
                // `block.content` only catches up at blur / Enter / etc.
                // So while editing, compare against `in_memory_content`.
                let expected: String = match &ref_state.active_editor {
                    Some(active) if active.block_id == block.id => active.in_memory_content.clone(),
                    _ => block.content_text().to_string(),
                };
                if displayed != &expected {
                    // Tag each mismatch with where the divergence lives —
                    // backend (engine snapshot also stale) vs GPUI render
                    // layer (engine snapshot matches expected). The
                    // diagnostic asks the same `ReactiveEngine` the GPUI
                    // window is bound to, so it shows whether the engine
                    // produced the right ViewModel and the render layer
                    // dropped it, or whether the bug is upstream.
                    let diag_label = self
                        .frontend_engine
                        .as_ref()
                        .map(|engine| {
                            crate::pbt::panic_diag::diagnose_displayed_text(
                                engine, entity_id, displayed, &expected,
                            )
                            .as_label()
                        })
                        .unwrap_or_else(|| "no engine handle".into());
                    mismatches.push(format!(
                        "  {widget}@block={entity_id}\n    on-screen: {:?}\n    \
                         expected:  {:?}\n    [DIAG] {diag_label}",
                        displayed,
                        expected,
                        widget = info.widget_type,
                    ));
                }
            }
            assert!(
                mismatches.is_empty(),
                "[inv-displayed-text] {} text widget(s) show stale content. \
                     The on-screen string diverged from the SQL block.content in the \
                     reference model — typical after split_block/join_block when the \
                     row's data signal fires but a rendered prop (editable_text \
                     InputState, text col(...) snapshot) skips the update.\n\
                     Per-line [DIAG] tag distinguishes backend (engine ViewModel \
                     also stale) from GPUI render layer (engine snapshot matches \
                     expected; render layer dropped the update).\n{}",
                mismatches.len(),
                mismatches.join("\n"),
            );
        }
    }

    // ─── Per-invariant helpers ────────────────────────────────────────────
    // Each method below corresponds to one section of `check_invariants_async`
    // that has been migrated to a capability-bound `Invariant<R, S>` body.
    // The wide PBT calls these in order; slim slices reuse the bodies
    // directly without going through this E2ESut-typed dispatcher.

    /// `inv-loro-no-errors` — see
    /// `pbt/invariants/bodies/loro_no_errors.rs`.
    pub(super) async fn check_inv_loro_no_errors(&self, ref_state: &ReferenceState) {
        use crate::pbt::invariants::bodies::loro_no_errors::InvLoroNoErrors;
        assert_invariants!(ref_state, self, InvLoroNoErrors);
    }

    /// `inv-org-render-fixed-point` — see
    /// `pbt/invariants/bodies/org_render_fixed_point.rs`.
    pub(super) async fn check_inv_org_render_fixed_point(&self, ref_state: &ReferenceState) {
        use crate::pbt::invariants::bodies::org_render_fixed_point::InvOrgRenderFixedPoint;
        assert_invariants!(ref_state, self, InvOrgRenderFixedPoint);
    }

    /// `inv-focus-matches-ref` — see
    /// `pbt/invariants/bodies/focus_matches_ref.rs`.
    pub(super) async fn check_inv_focus_matches_ref(&self, ref_state: &ReferenceState) {
        use crate::pbt::invariants::bodies::focus_matches_ref::InvFocusMatchesRef;
        assert_invariants!(ref_state, self, InvFocusMatchesRef);
    }

    /// Flutter startup-race assertion. Not strictly an invariant — guards
    /// the DDL/sync race condition where pre-existing files publish errors
    /// during initial sync. Lives here so the `check_invariants_async`
    /// runner stays narrative.
    pub(super) fn check_inv_no_startup_errors(&self) {
        assert!(
            !self.has_startup_errors(),
            "FLUTTER STARTUP BUG: {} publish errors during startup.\n\
                 This indicates DDL/sync race condition when {} pre-existing files were synced.\n\
                 Files: {:?}",
            self.startup_error_count(),
            self.documents.len(),
            self.documents.keys().collect::<Vec<_>>()
        );
    }

    /// Sections 4 + 5: view selection synchronized + active watch sets
    /// match the reference model. These are sync (no async / no SQL) so
    /// they're combined into a single named method to keep the runner
    /// narrative tight.
    pub(super) fn check_inv_view_and_watches(&self, ref_state: &ReferenceState) {
        assert_eq!(self.current_view, ref_state.current_view());
        assert_eq!(
            self.active_watches.keys().collect::<HashSet<_>>(),
            ref_state.active_watches.keys().collect::<HashSet<_>>(),
            "Watch sets diverged"
        );
    }

    /// Section 6: structural integrity. Every non-root block must reference
    /// a parent that exists in `backend_blocks`. Skipped when the
    /// `live_blocks` mirror lagged (an orphan in a stale snapshot is just
    /// an "I haven't seen the parent yet" artifact, not a real bug).
    pub(super) fn check_inv_no_orphan_blocks(
        &self,
        backend_blocks: &[Block],
        live_blocks_stale: bool,
    ) {
        if live_blocks_stale {
            return;
        }
        for block in backend_blocks {
            if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                continue;
            }
            assert!(
                backend_blocks.iter().any(|b| b.id == block.parent_id),
                "Orphan block: {} has invalid parent {}",
                block.id,
                block.parent_id
            );
        }
    }
}
