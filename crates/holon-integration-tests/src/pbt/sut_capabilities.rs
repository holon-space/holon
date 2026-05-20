//! Blanket `SutLoro` + `SutLoroLog` + `SutLifecycle` + Phase 6c-g impls on `E2ESut`.
//!
//! Follows the same pattern as `reference_capabilities.rs`:
//! thin forwarding impls that expose capability-trait surface
//! over existing inherent / `SutHandle` methods.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use holon_pbt_core::capabilities::{
    EngineFocus, EntityUri, FrontendRootVm, PeerEditOp, ProviderStabilityReport, RenderedElement,
    SutBackend, SutBlockTreeWrite, SutCdc, SutDriver, SutEditorMirrorRead, SutEditorMirrorWrite,
    SutLayout, SutLifecycle, SutLoro, SutLoroLog, SutLoroTaskState, SutOrgFileWrite, SutOrgRead,
    SutOrgRender, SutQueryCompile, SutRenderer, SutSqlProjection, SutViewModel, SutWatchRows,
    TextOp, ViewportHint, WatchRow,
};

use super::sut::E2ESut;
use super::transition_dispatch::SutHandle;
use holon_frontend::reactive::BuilderServices;

// ─── SutLoro (forwarding) ─────────────────────────────────────────────
//
// `E2ESut` owns the `LoroSut` (`self.loro_sut`) but the dispatch macro
// fixes one `S = E2ESut` for every transition, so peer transitions reach
// the peer surface through this thin forward. All logic + state lives in
// `LoroSut`; each method here just delegates. `SutHandle: SutLoro`, so this
// impl is what satisfies that supertrait bound on `E2ESut`.

impl E2ESut {
    /// The owned peer surface. Panics if Loro is disabled — peer transitions
    /// gate on `enable_loro` in their preconditions, so reaching here without
    /// `loro_sut` is a wiring bug, not a runtime condition.
    fn loro_mut(&mut self) -> &mut crate::pbt::sut_loro::LoroSut {
        self.loro_sut
            .as_mut()
            .expect("SutLoro op reached E2ESut but Loro is not enabled (loro_sut is None)")
    }
}

#[allow(async_fn_in_trait)]
impl SutLoro for E2ESut {
    async fn apply_add_peer(&mut self) {
        self.loro_mut().apply_add_peer().await;
    }

    async fn apply_peer_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        self.loro_mut()
            .apply_peer_create(peer_idx, parent_stable_id, content, stable_id)
            .await;
    }

    async fn apply_peer_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
        self.loro_mut()
            .apply_peer_update(peer_idx, stable_id, content)
            .await;
    }

    async fn apply_peer_delete(&mut self, peer_idx: usize, stable_id: &str) {
        self.loro_mut().apply_peer_delete(peer_idx, stable_id).await;
    }

    async fn apply_peer_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    ) {
        self.loro_mut()
            .apply_peer_char_insert(peer_idx, stable_id, pos_codepoint, text)
            .await;
    }

    async fn apply_peer_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    ) {
        self.loro_mut()
            .apply_peer_char_delete(peer_idx, stable_id, pos_codepoint, len_codepoint)
            .await;
    }

    async fn apply_peer_edit(&mut self, peer_idx: usize, op: &PeerEditOp) {
        self.loro_mut().apply_peer_edit(peer_idx, op).await;
    }

    async fn apply_peer_char_edit(&mut self, peer_idx: usize, block_id: &str, op: &TextOp) {
        self.loro_mut()
            .apply_peer_char_edit(peer_idx, block_id, op)
            .await;
    }

    async fn apply_sync_with_peer(&mut self, peer_idx: usize) {
        self.loro_mut().apply_sync_with_peer(peer_idx).await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        self.loro_mut().apply_merge_from_peer(peer_idx).await;
    }

    async fn apply_create_stale_peer(&mut self, lag_steps: usize) {
        self.loro_mut().apply_create_stale_peer(lag_steps).await;
    }
}

// ─── SutLifecycle ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutLifecycle for E2ESut {
    async fn apply_start_app(&mut self) {
        self.ctx
            .start_app(true)
            .await
            .expect("SutLifecycle::apply_start_app failed");
    }

    /// Triggers a restart simulation with an empty expected-ids set. The
    /// wide-PBT transition passes the full expected set from `ref_state`;
    /// lifecycle callers that don't have ref_state use the empty set so
    /// `simulate_restart` still clears and re-syncs Loro without blocking
    /// on block convergence.
    async fn apply_simulate_restart(&mut self) {
        self.ctx
            .simulate_restart(&HashSet::new())
            .await
            .expect("SutLifecycle::apply_simulate_restart failed");
    }

    async fn is_app_started(&self) -> bool {
        self.ctx.is_running()
    }
}

// ─── SutLoroLog ───────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutLoroLog for E2ESut {
    /// Delegates to `TestContext::loro_sync_error_count`, which reads the
    /// `LoroSyncController`'s atomic error counter. Same source as the
    /// `inv-loro-no-errors` body.
    async fn loro_had_errors(&self) -> bool {
        self.ctx.loro_sync_error_count() > 0
    }

    /// Ordered child block ids of `block_id` in the live Loro tree.
    /// `None` when Loro isn't enabled or `block_id` is absent from the tree;
    /// `Some(vec)` (possibly empty) when the node exists. Reads the same
    /// `LoroBackend::get_all_blocks` snapshot `LoroSut` trusts, filters by
    /// `parent_id`, and orders by the fractional-index `sort_key` — the
    /// authoritative sibling order.
    async fn loro_children_of(&self, block_id: &str) -> Option<Vec<String>> {
        let snaps = self
            .loro_sut
            .as_ref()?
            .read_block_snapshots()
            .await
            .unwrap_or_else(|e| panic!("SutLoroLog::loro_children_of: read failed: {e}"));
        snaps.iter().find(|s| s.block.id.as_str() == block_id)?;
        let mut children: Vec<_> = snaps
            .iter()
            .filter(|s| s.block.parent_id.as_str() == block_id)
            .collect();
        children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        Some(
            children
                .into_iter()
                .map(|s| s.block.id.to_string())
                .collect(),
        )
    }

    /// Every block in the live Loro tree (`read_blocks`), or `None` when Loro
    /// isn't enabled on this variant. `read_blocks` failures panic — a Loro
    /// read error is itself a bug, never silently swallowed.
    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>> {
        let loro = self.loro_sut.as_ref()?;
        Some(
            loro.read_blocks().await.unwrap_or_else(|e| {
                panic!("SutLoroLog::loro_block_snapshot: read_blocks failed: {e}")
            }),
        )
    }
}

// ─── SutErrorLog ──────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl holon_pbt_core::capabilities::SutErrorLog for E2ESut {
    /// Flutter/event publish errors logged during initial document sync.
    async fn app_error_count(&self) -> usize {
        self.startup_error_count()
    }

    /// The documents present at startup — context for the failure message.
    async fn app_error_context(&self) -> Vec<String> {
        self.documents.keys().map(|k| k.to_string()).collect()
    }
}

// ─── SutLoroTaskState ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutLoroTaskState for E2ESut {
    /// Project a block's `task_state` from the live Loro tree. `None` when
    /// Loro isn't enabled, the block is absent, or it has no `task_state`
    /// property. Reads the same `properties["task_state"]` scalar the SQL
    /// sibling [`SutSqlProjection::block_task_state`] reads via
    /// `json_extract(properties,'$.task_state')`, so the two are directly
    /// comparable by `inv-task-state-storage-coherence`.
    async fn loro_task_state_of(&self, block_id: &str) -> Option<String> {
        let blocks = self
            .loro_sut
            .as_ref()?
            .read_blocks()
            .await
            .unwrap_or_else(|e| {
                panic!("SutLoroTaskState::loro_task_state_of: read_blocks failed: {e}")
            });
        let block = blocks.iter().find(|b| b.id.as_str() == block_id)?;
        block
            .properties
            .get("task_state")
            .and_then(|v| v.as_string_owned())
    }
}

// ─── SutSqlProjection ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutSqlProjection for E2ESut {
    /// Queries the `block` materialized view for a single row and returns its
    /// fields as strings. Same data path as the
    /// `inv-blocks-match-ref/matview` body's matview read.
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let sql = format!("SELECT * FROM block WHERE id = '{escaped}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_row query failed");
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    /// Returns all non-deleted block IDs from `block_raw`. Mirrors the
    /// `SELECT id FROM block_raw` convergence poll in
    /// `E2ESut::settle_before_invariants`.
    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        let rows = self
            .ctx
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("SutSqlProjection::all_block_ids query failed");
        rows.into_iter()
            .filter_map(|r| {
                r.get("id").and_then(|v| v.as_string()).map(|s| {
                    EntityUri::parse(s).expect("block id from SQL must be a valid EntityUri")
                })
            })
            .collect()
    }

    /// `SELECT id FROM block_raw WHERE parent_id = ? ORDER BY sort_key, id`
    /// — the SQL projection's per-parent sibling order, the same ordering the
    /// org renderer and UI tree consume. Compared against the ref model's
    /// `sorted_children` by `inv-live-children-match-ref`.
    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let escaped = parent.as_str().replace('\'', "''");
        let sql =
            format!("SELECT id FROM block_raw WHERE parent_id = '{escaped}' ORDER BY sort_key, id");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::sorted_children query failed");
        rows.into_iter()
            .filter_map(|r| {
                r.get("id").and_then(|v| v.as_string()).map(|s| {
                    EntityUri::parse(s).expect("block id from SQL must be a valid EntityUri")
                })
            })
            .collect()
    }

    /// Returns the current row count for a registered CDC watch via
    /// `TestContext::ui_model`. Returns `None` when the query_id is not
    /// registered.
    async fn watch_row_count(&self, query_id: &str) -> Option<usize> {
        self.ctx.ui_model.get(query_id).map(|acc| acc.len())
    }

    /// Queries `block_raw` (write-side base table, no matview hydration) for a
    /// single row. Used by the WARN/SKIP CDC-lag classifier in the
    /// `inv-blocks-match-ref/block_raw` body.
    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let sql = format!("SELECT * FROM block_raw WHERE id = '{escaped}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_raw_row query failed");
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    /// `block_raw.content` for `id`. Returns `None` if the block doesn't
    /// exist. Used by the split-block content-routing slice
    /// (`inv-block-content-matches-ref`).
    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let sql = format!("SELECT content FROM block_raw WHERE id = '{escaped}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_content query failed");
        rows.into_iter().next().and_then(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
    }

    /// Rows of the `current_focus` matview as `(region, block_id)`. `block_id`
    /// is `None` when the column is NULL (region navigated home). Mirrors the
    /// inline §7 `SELECT region, block_id FROM current_focus`.
    async fn current_focus_rows(&self) -> Vec<(String, Option<String>)> {
        let rows = self
            .ctx
            .query_sql("SELECT region, block_id FROM current_focus")
            .await
            .expect("SutSqlProjection::current_focus_rows query failed");
        rows.into_iter()
            .filter_map(|r| {
                let region = r.get("region").and_then(|v| v.as_string())?.to_string();
                let block_id = r
                    .get("block_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string);
                Some((region, block_id))
            })
            .collect()
    }

    /// Rows of the `focus_roots` matview as `(region, root_id)`. Mirrors the
    /// inline §8 truth-check `SELECT region, root_id FROM focus_roots`.
    async fn focus_roots_rows(&self) -> Vec<(String, String)> {
        let rows = self
            .ctx
            .query_sql("SELECT region, root_id FROM focus_roots")
            .await
            .expect("SutSqlProjection::focus_roots_rows query failed");
        rows.into_iter()
            .filter_map(|r| {
                let region = r.get("region").and_then(|v| v.as_string())?.to_string();
                let root_id = r.get("root_id").and_then(|v| v.as_string())?.to_string();
                Some((region, root_id))
            })
            .collect()
    }

    async fn nav_history_open_rows(&self) -> Vec<(String, String)> {
        // The exact predicate the focus_roots matview projects from, read off
        // the BASE table so inv-focus-roots can tell matview/IVM drift from a
        // holon close-path bug.
        let rows = self
            .ctx
            .query_sql(
                "SELECT region, block_id FROM navigation_history \
                 WHERE closed_at IS NULL AND block_id IS NOT NULL",
            )
            .await
            .expect("SutSqlProjection::nav_history_open_rows query failed");
        rows.into_iter()
            .filter_map(|r| {
                let region = r.get("region").and_then(|v| v.as_string())?.to_string();
                let block_id = r.get("block_id").and_then(|v| v.as_string())?.to_string();
                Some((region, block_id))
            })
            .collect()
    }

    /// Returns all distinct block_id values from `block_tags`. Mirrors
    /// `SELECT DISTINCT block_id FROM block_tags` — same table used by
    /// `inv-block-tags-references-exist`.
    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        let rows = self
            .ctx
            .query_sql("SELECT DISTINCT block_id FROM block_tags")
            .await
            .expect("SutSqlProjection::block_tag_block_ids query failed");
        rows.into_iter()
            .filter_map(|r| {
                r.get("block_id").and_then(|v| v.as_string()).map(|s| {
                    EntityUri::parse(s).expect("block_tags.block_id must be a valid EntityUri")
                })
            })
            .collect()
    }

    /// Reads `json_extract(properties, '$.task_state')` from `block_raw`
    /// for the given block id. Returns `None` when the block doesn't exist
    /// or the property is absent/null.
    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let sql = format!(
            "SELECT json_extract(properties, '$.task_state') AS task_state \
             FROM block_raw WHERE id = '{escaped}'"
        );
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_task_state query failed");
        rows.into_iter().next().and_then(|r| {
            r.get("task_state")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
    }
}

// ─── SutBackend ───────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutBackend for E2ESut {
    /// Snapshot the CDC-driven `block` matview mirror (`live_blocks`) as
    /// typed `Block` values via `live_blocks().read().values().cloned()`.
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.live_blocks()
            .await
            .read()
            .values()
            .map(|b| (**b).clone())
            .collect()
    }

    /// Read the write-side `block_raw` table directly into `Block` values.
    /// Only `block_raw`'s native columns are selected (no junction `tags`/
    /// `requires`); `parse_block_row` leaves those empty, so the
    /// `/block_raw` store compares a field subset. Panics on a row that
    /// won't parse — a malformed `block_raw` row is a bug, not skipped.
    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block> {
        let rows = self
            .ctx
            .query_sql(
                "SELECT id, parent_id, content, content_type, source_language, properties \
                 FROM block_raw",
            )
            .await
            .expect("SutBackend::block_raw_snapshot query failed");
        rows.iter()
            .map(|r| {
                let r: holon::storage::types::StorageEntity = r
                    .iter()
                    .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v.clone()))
                    .collect();
                super::sut_row_parsing::parse_block_row(&r).unwrap_or_else(|| {
                    panic!("block_raw_snapshot: parse_block_row returned None for row {r:?}")
                })
            })
            .collect()
    }

    /// Snapshot the CDC-driven `focus_roots` mirror (`live_focus_roots`) as
    /// `(region, root_id)` rows via `live_focus_roots().read().values()`.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.live_focus_roots()
            .await
            .read()
            .values()
            .map(|fr| (fr.region.clone(), fr.root_id.clone()))
            .collect()
    }
}

// ─── SutWatchRows ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutWatchRows for E2ESut {
    /// `ui_model` keys — the watches currently registered on the SUT. The
    /// active watch query-id map.
    async fn watch_query_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.ctx.ui_model.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// CDC-delivered rows for `query_id` (`ui_model[query_id].to_vec()`),
    /// each `Value` mapped through `as_string()` into the `WatchRow` shape.
    /// Empty if the watch is not registered.
    async fn watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        let Some(acc) = self.ctx.ui_model.get(query_id) else {
            return Vec::new();
        };
        acc.to_vec()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(k, v)| (k.to_string(), v.as_string().map(str::to_string)))
                    .collect()
            })
            .collect()
    }

    /// Runs the `block_raw` truth-check SQL and projects the `id` column.
    /// Extracts `id` from each row. Panics on a query error — a failed truth
    /// query is itself a bug, never silently swallowed.
    async fn block_raw_query_ids(&self, sql: &str) -> BTreeSet<EntityUri> {
        let rows = self.ctx.query_sql(sql).await.unwrap_or_else(|e| {
            panic!(
                "[inv-watch-rows-match-ref truth check] block_raw query failed\n\
                 sql: {sql}\n\
                 error: {e}"
            )
        });
        rows.into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| EntityUri::parse(s).expect("invalid entity URI in block_raw row"))
            })
            .collect()
    }

    /// `SELECT {field} FROM block_raw WHERE id = ?` — the per-field truth
    /// read for the field-level CDC-lag classifier.
    async fn block_raw_field(&self, id: &EntityUri, field: &str) -> Option<String> {
        let escaped_id = id.as_str().replace('\'', "''");
        let sql = format!("SELECT {field} FROM block_raw WHERE id = '{escaped_id}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutWatchRows::block_raw_field query failed");
        rows.into_iter()
            .next()
            .and_then(|r| r.get(field).and_then(|v| v.as_string()).map(str::to_string))
    }
}

// ─── SutOrgFileWrite ──────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutOrgFileWrite for E2ESut {
    /// Delegates to `SutHandle::apply_write_org_file`, which writes the file
    /// via `TestContext::write_org_file` and — when the app is running — waits
    /// for `FileSyncController` to ingest it and re-key `ctx.documents`.
    async fn write_org_file(&mut self, path: &str, contents: &str) {
        <E2ESut as SutHandle>::apply_write_org_file(self, path, contents).await;
    }
}

// ─── SutCdc ───────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutCdc for E2ESut {
    /// True when the `live_blocks` CDC mirror has not yet consumed all events
    /// emitted since the last write. Delegates to `E2ESut::live_blocks_cdc_stale`
    /// (pub(super) accessor): compares `LiveData::consumed_seq()` against
    /// `db_handle().cdc_emitted_watermark()`. Returns `false` pre-startup or
    /// before the mirror is initialised.
    async fn cdc_in_flight(&self) -> bool {
        self.live_blocks_cdc_stale()
    }

    /// Drains pending CDC events from all active watches into the `ui_model`.
    /// Delegates to `TestContext::drain_cdc_events` — same logic used between
    /// transitions in `apply_transition_async`.
    async fn drain_cdc(&mut self) {
        self.ctx.drain_cdc_events().await;
    }
}

// ─── SutViewModel ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutViewModel for E2ESut {
    /// Drains all pending ViewModel emissions since the last call.
    /// Delegates to `E2ESut::vm_emissions_drain` (pub(super) accessor).
    /// Each ViewModel is rendered via `pretty_print(0)` — the same format
    /// used by `render_tree_of` for consistent slice-level string comparisons.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        self.vm_emissions_drain()
            .into_iter()
            .map(|vm| vm.pretty_print(0))
            .collect()
    }

    /// True if the frontend root ViewModel is the Error variant.
    /// Mirrors `inv-frontend-root-not-error` (sut.rs:6038–6047):
    /// snapshots the root URI via the frontend engine and checks
    /// `widget_name() == "error"`. Uses `frontend_engine` (pub) and falls
    /// back to `false` when none is installed. `reactive_root_id` (private)
    /// is not reachable here, so uses `root_layout_block_uri` as the root.
    async fn frontend_root_is_error(&self) -> bool {
        let Some(engine) = self.render.frontend_engine.clone() else {
            return false;
        };
        let root_uri = holon_api::root_layout_block_uri();
        let vm = engine.snapshot(&root_uri);
        vm.widget_name() == Some("error")
    }

    /// Snapshot the headless `ReactiveEngine`'s rendered ViewModel and
    /// count Error widgets (the `inv-viewmodel-no-error-widgets` path).
    /// Returns `None` when the engine isn't installed,
    /// the root id isn't set yet, the render expression is still loading /
    /// placeholder, or shadow interpretation panics.
    async fn headless_error_node_count(&self) -> Option<usize> {
        let engine = self.render.reactive_engine.borrow().clone()?;
        let root_id = self.render.reactive_root_id.borrow().clone()?;
        let results = engine.ensure_watching(&root_id);
        if results.is_loading() {
            return None;
        }
        let (render_expr, data_rows) = results.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return None;
        }
        // Independent (fresh) headless re-interpret: Turso uses a fresh
        // `HeadlessBuilderServices` over its `BackendEngine`, no-Turso the
        // reactive engine over `block_query` (see `render_builder_services`).
        let services = self.render_builder_services();
        let _ = engine; // hold ref until snapshot completes
        let re = render_expr.clone();
        let dr = data_rows.clone();
        let tree = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                holon_frontend::interpret_pure(&re, &dr, &*services).snapshot()
            }))
        })
        .await
        .ok()?
        .ok()?;
        Some(holon_layout_testing::display_assertions::count_error_nodes(
            &tree,
        ))
    }

    /// The selected view mode tracked on the test context.
    async fn current_view(&self) -> String {
        self.current_view.clone()
    }

    /// Resolve the frontend engine's root layout and return its widget kind +
    /// ordered entity ids. `None` when no frontend engine is installed or the
    /// root is still loading (mirrors the inline `inv-frontend-engine` gate at
    /// sut_check_invariants.rs: ensure_watching → is_loading skip → snapshot).
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm> {
        let engine = self.render.frontend_engine.clone()?;
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = engine.ensure_watching(&root_uri);
        if rqr.is_loading() {
            engine.unwatch(&root_uri);
            return None;
        }
        let vm = engine.snapshot(&root_uri);
        let root_kind = vm.widget_name().unwrap_or("?").to_string();
        let entity_ids = vm
            .collect_entity_ids()
            .into_iter()
            // ALLOW(entity_uri_from_raw): vm.collect_entity_ids() Vec<String> from rendered ViewModel
            .map(|s| EntityUri::from_raw(&s))
            .collect();
        engine.unwatch(&root_uri);
        Some(FrontendRootVm {
            root_kind,
            entity_ids,
        })
    }

    /// Force `viewport`, interpret the reactive root layout twice, and report
    /// on the streaming providers. Port of the inline value-fn block
    /// (sut_check_invariants.rs `[vfn11/12/13]` + `[inv_bar]`); the
    /// `ReactiveEngine`/`interpret_pure`/`ProviderCache` coupling stays here,
    /// the assertions move to the body.
    async fn provider_stability_report(
        &self,
        viewport: ViewportHint,
    ) -> Option<ProviderStabilityReport> {
        use crate::pbt::value_fn_invariants::{
            collect_providers, count_bottom_docks, rhai_mentions,
        };
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc;

        let reactive = self.render.reactive_engine.borrow().clone()?;

        // Render observation and the driver now share one engine (see
        // `sut.rs::ensure_reactive_engine`), so `focus_chain()` reads the SAME
        // focus the driver set — no cross-engine mirroring needed. The probe
        // viewport below is narrow (forces the `if_space`-gated mobile bar), so
        // save and restore the engine's real viewport around the probe to avoid
        // perturbing later render observations on the shared engine.
        let prev_viewport = reactive.ui_state().viewport();
        reactive
            .ui_state()
            .set_viewport(holon_frontend::reactive::ViewportInfo {
                width_px: viewport.width_px,
                height_px: viewport.height_px,
                scale_factor: 1.0,
            });
        tokio::task::yield_now().await;

        let root_id = self
            .render
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        let results = reactive.ensure_watching(&root_id);
        let (render_expr, data_rows) = results.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return None;
        }

        let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();

        // Pass 1.
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
        .expect("spawn_blocking panicked")?;

        let providers1 = collect_providers(&tree1);
        let total_providers = providers1.len();
        let mentions_bottom_dock = rhai_mentions(&render_expr, "bottom_dock");
        let bottom_dock_count = if mentions_bottom_dock {
            count_bottom_docks(&tree1)
        } else {
            0
        };
        let mentions_focus_chain = rhai_mentions(&render_expr, "focus_chain");
        let any_nonempty = providers1.iter().any(|p| p.rows_snapshot_len > 0);

        // vfn12: provider identity stability within one pass — group by
        // (template, rows) and require a single cache_identity per group.
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
        let identity_instability = ids_per_group.iter().find_map(|(key, ids)| {
            (ids.len() > 1).then(|| {
                let sites = sites_per_group.get(key).copied().unwrap_or(0);
                format!(
                    "template={} rows={} → {} distinct cache_identities across {sites} call sites",
                    key.0,
                    key.1,
                    ids.len(),
                )
            })
        });

        // vfn13: cache identity flicker across re-interpret. A pass-2 panic
        // leaves flicker unmeasured (0) rather than failing the report.
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
        let flicker_count = match tree2 {
            Some(tree2) => {
                let providers2 = collect_providers(&tree2);
                let ids1: HashSet<u64> = providers1.iter().map(|p| p.cache_identity).collect();
                let ids2: HashSet<u64> = providers2.iter().map(|p| p.cache_identity).collect();
                ids1.difference(&ids2).count()
            }
            None => 0,
        };

        // Restore the engine's real viewport so the narrow probe doesn't leak
        // into later render observations on the shared engine. (Headless has no
        // viewport — `None` — so this is a no-op there; it matters only when a
        // real frontend set one.)
        if let Some(v) = prev_viewport {
            reactive.ui_state().set_viewport(v);
        }

        Some(ProviderStabilityReport {
            mentions_bottom_dock,
            bottom_dock_count,
            mentions_focus_chain,
            total_providers,
            any_nonempty,
            identity_instability,
            flicker_count,
        })
    }

    /// Drain the per-transition ViewModel emission buffer and extract every
    /// `StateToggle` node's `(block_id, current)`. Port of the inline
    /// `[inv-value-fn-provider-identity]` intermediate-emissions check
    /// (sut_check_invariants.rs); the body compares each against the reference.
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)> {
        let emissions: Vec<holon_frontend::ViewModel> =
            std::mem::take(&mut *self.render.vm_emissions.lock().unwrap());
        let mut out = Vec::new();
        for vm in &emissions {
            for toggle in crate::display_assertions::collect_state_toggle_nodes(vm) {
                if let holon_frontend::view_model::ViewKind::StateToggle { current, .. } =
                    &toggle.kind
                    && let Some(block_id_str) = toggle.row_id()
                {
                    // ALLOW(entity_uri_from_raw): toggle.row_id() String from ViewModel StateToggle node
                    out.push((EntityUri::from_raw(&block_id_str), current.clone()));
                }
            }
        }
        out
    }

    /// Compare the persistent `HeadlessLiveTree` (the collection driver's
    /// `set_data` path) against a fresh interpretation of the same data rows.
    /// Port of the inline `[inv10h_live]` block (former
    /// sut_check_invariants.rs §10); the `ReactiveEngine`/`HeadlessLiveTree`
    /// coupling stays SUT-side, the assertion moves to the body.
    ///
    /// Anchored on the main-panel block, not the root: the root layout has a
    /// render expression but no data query (its rows are always empty), while
    /// the collection driver actually runs on the nested main panel.
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>> {
        use futures::StreamExt;
        use std::sync::Arc;
        use std::time::Duration;

        let root_id = self
            .render
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        self.ensure_reactive_engine(&root_id).await;
        let reactive = self.render.reactive_engine.borrow().clone()?;

        let main_panel_id = holon_api::EntityUri::block("default-main-panel");
        let mp_results = reactive.ensure_watching(&main_panel_id);

        // Wait for the main-panel watcher to deliver its first non-loading,
        // non-empty emission. ToggleState only fires after a sidebar click
        // populates focus_roots, so the watcher may still be cold on the first
        // ClickBlock-only transition; give up after 2s.
        {
            let mut mp_stream = reactive.watch(&main_panel_id);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let (mp_render, mp_rows) = mp_results.snapshot();
                let still_loading = matches!(
                    &mp_render,
                    holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading"
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
        if still_loading || mp_data_rows.is_empty() {
            return None;
        }

        let item_template =
            holon_layout_testing::live_tree::extract_item_template(&mp_render_expr)?;

        // Lazily init the persistent live tree on first use. The layout MUST
        // mirror prod's main panel (derived from the real render expr): a
        // hierarchical panel runs `create_tree_driver` + its targeted focus
        // driver, the path where the focus variant swap can freeze. Forcing a
        // flat `list` here masked that whole class of bug.
        if self.render.live_tree.borrow().is_none() {
            let data_source: Arc<dyn holon_api::ReactiveRowProvider> = mp_results.clone();
            let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
            // Mirror prod's main panel: derive the ACTIVE collection variant
            // (resolves `view_mode_switcher` to its default mode, normally the
            // hierarchical `tree`). Falling back to a flat `list` only when no
            // collection is present keeps non-hierarchical panels working.
            let layout =
                holon_layout_testing::live_tree::extract_collection_variant(&mp_render_expr)
                    .unwrap_or_else(|| {
                        holon_frontend::reactive_view_model::CollectionVariant::list(0.0)
                    });
            let lt = holon_layout_testing::live_tree::HeadlessLiveTree::new(
                data_source,
                item_template.clone(),
                layout,
                services,
                &reactive.runtime_handle,
            );
            *self.render.live_tree.borrow_mut() = Some(lt);
            // Give the driver time to populate initial items.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Let the driver process pending VecDiff events.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let live_ref = self.render.live_tree.borrow();
        let lt = live_ref.as_ref()?;
        let live_items = lt.items();
        // Match live↔fresh by ROW ID, not position: a hierarchical live tree
        // projects rows in DFS order, which can differ from the matview order
        // `mp_data_rows` follows. Pairing by id compares the same row on both
        // sides regardless of order. The id MUST come from the source row
        // (`mp_data_rows`) — a freshly interpreted VM does not carry the row id
        // in `.data`, so keying the fresh side off the VM would pair nothing.
        // Rows present on only one side (a not-yet-applied InsertAt/RemoveAt)
        // are skipped — the bug we catch is a stale VARIANT/props on a row
        // present in both.
        let row_id_of = |vm: &holon_frontend::ReactiveViewModel| -> Option<String> {
            vm.data
                .get_cloned()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        };
        let fresh_by_id: std::collections::HashMap<String, Arc<holon_frontend::ReactiveViewModel>> =
            mp_data_rows
                .iter()
                .filter_map(|row| {
                    let id = row.get("id").and_then(|v| v.as_string())?.to_string();
                    let ctx = holon_frontend::RenderContext::default().with_row(row.clone());
                    Some((id, Arc::new(reactive.interpret(&item_template, &ctx))))
                })
                .collect();
        let mut prop_diffs = Vec::new();
        // The tree driver wraps each row in a `tree_item` (depth/has_children)
        // for indentation; the fresh side interprets the bare `item_template`
        // and has no such wrapper. Compare like-for-like by unwrapping the
        // live `tree_item` to its content child — otherwise every row trips a
        // spurious `tree_item` vs `column` mismatch that also masks the nested
        // variant divergence we actually want to catch.
        let unwrap_tree_item =
            |vm: &Arc<holon_frontend::ReactiveViewModel>| -> Arc<holon_frontend::ReactiveViewModel> {
                if vm.widget_name().as_deref() == Some("tree_item") {
                    if let Some(child) = vm.children.first() {
                        return child.clone();
                    }
                }
                vm.clone()
            };
        for live in &live_items {
            let Some(row_id) = row_id_of(live) else {
                continue;
            };
            let Some(fresh) = fresh_by_id.get(&row_id) else {
                continue;
            };
            let live_cmp = unwrap_tree_item(live);
            for d in crate::display_assertions::tree_diff(live_cmp.as_ref(), fresh.as_ref()) {
                prop_diffs.push(format!("  {row_id}: {d}"));
            }
        }

        // ORDER check: the live tree must render each parent's children in
        // document order (`sort_key`, the authoritative fractional index that
        // also matches the reference model). The reactive row set keys rows by
        // `EntityUri` in a BTreeMap, so a naive projection surfaces them in
        // *block-id* order — a split mints a random-UUID block, which then
        // renders at its UUID's lexicographic rank instead of right after its
        // source. We compare PER PARENT (not the flat DFS list, whose
        // parent-interleaving is legitimate) so hierarchy never false-positives.
        // The correct order is derived from `sort_key` carried on the same rows
        // (`SELECT *`), so this stays a pure `SutViewModel` check.
        let parent_of = |id: &str| -> Option<String> {
            mp_data_rows.iter().find_map(|r| {
                (r.get("id").and_then(|v| v.as_string()) == Some(id)).then(|| {
                    r.get("parent_id")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string()
                })
            })
        };
        let sort_key_of = |id: &str| -> Option<String> {
            mp_data_rows.iter().find_map(|r| {
                (r.get("id").and_then(|v| v.as_string()) == Some(id)).then(|| {
                    r.get("sort_key")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string()
                })
            })
        };
        let live_order: Vec<String> = live_items.iter().filter_map(|v| row_id_of(v)).collect();
        // Group the live order by parent, preserving live order within each group.
        let mut by_parent: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for id in &live_order {
            if let Some(parent) = parent_of(id) {
                by_parent.entry(parent).or_default().push(id.clone());
            }
        }
        for (parent, live_sibs) in &by_parent {
            if live_sibs.len() < 2 {
                continue; // a lone child can't be mis-ordered
            }
            // `sort_key` is required to know the intended order; if any sibling
            // lacks one, skip rather than fabricate an order (disclosed: no
            // false pass — the prop diffs above still run).
            let keys: Vec<(String, String)> = live_sibs
                .iter()
                .map(|id| (sort_key_of(id).unwrap_or_default(), id.clone()))
                .collect();
            if keys.iter().any(|(k, _)| k.is_empty()) {
                continue;
            }
            let mut want = keys.clone();
            want.sort(); // by (sort_key, id)
            let want_ids: Vec<&String> = want.iter().map(|(_, id)| id).collect();
            let live_ids: Vec<&String> = live_sibs.iter().collect();
            if live_ids != want_ids {
                prop_diffs.push(format!(
                    "  ORDER under parent {parent}: live renders {live_ids:?} but sort_key \
                     order is {want_ids:?} — the reactive collection is not ordering by sort_key \
                     (the fractional-index authority)"
                ));
            }
        }

        Some(prop_diffs)
    }
}

// ─── SutRenderer ──────────────────────────────────────────────────────

impl E2ESut {
    /// Resolve a ready reactive watch for `uri`.
    ///
    /// With an installed `frontend_engine` (phased/GPUI harness) this keeps the
    /// original semantics: return `None` immediately if the watch is still
    /// loading. Without one (the `declare_pbt_slice!` harness has no GPUI), it
    /// falls back to the lazily-created headless `reactive_engine` and polls
    /// until its first results load (or a short timeout), since that engine
    /// fills from background tasks on the shared runtime. This lets widget
    /// assertions render headlessly without changing the frontend path.
    async fn resolve_watch(
        &self,
        uri: &holon_api::EntityUri,
    ) -> Option<std::sync::Arc<holon_frontend::reactive::ReactiveRenderedRows>> {
        if let Some(engine) = self.render.frontend_engine.clone() {
            let rqr = engine.ensure_watching(uri);
            return (!rqr.is_loading()).then_some(rqr);
        }
        self.ensure_reactive_engine(uri).await;
        let engine = self.render.reactive_engine.borrow().clone()?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let rqr = engine.ensure_watching(uri);
            if !rqr.is_loading() {
                return Some(rqr);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Builder services for an *independent* (fresh) headless re-interpret in the
    /// render invariants. The Turso wiring re-interprets through a fresh
    /// `HeadlessBuilderServices` over its `BackendEngine` (preserving the
    /// established independence from the live reactive state); the no-Turso wiring
    /// has no engine, so it re-interprets through the reactive engine built over
    /// `block_query` — the only builder-services it carries (the same handle the
    /// `SutViewModel` methods already use). Callers run `resolve_watch` first, so
    /// the reactive engine is populated by here. `HeadlessBuilderServices` is
    /// Turso-only by construction, so the selection keys off the explicit storage
    /// backend, not a capability-presence proxy.
    fn render_builder_services(&self) -> std::sync::Arc<dyn BuilderServices> {
        match self.ctx.storage() {
            holon::di::StorageSelector::Turso => std::sync::Arc::new(
                holon_app::HeadlessBuilderServices::new(self.engine().clone()),
            ),
            holon::di::StorageSelector::LoroMemory => {
                self.render.reactive_engine.borrow().clone().expect(
                    "render_builder_services: no-Turso reactive engine must be set \
                     (resolve_watch runs first)",
                ) as std::sync::Arc<dyn BuilderServices>
            }
        }
    }
}

#[allow(async_fn_in_trait)]
impl SutRenderer for E2ESut {
    /// Returns a debug-formatted render-tree string for `id`. Uses the
    /// installed `frontend_engine` when present, else the headless
    /// `reactive_engine` fallback (see [`E2ESut::resolve_watch`]).
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String> {
        let rqr = self.resolve_watch(id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.render_builder_services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(vm.pretty_print(0))
    }

    /// Build a frontend-agnostic [`WidgetSnapshot`] from the ViewModel
    /// rooted at the layout root. Mirrors the path
    /// `render_tree_of` uses (interpret_pure against the layout root) but
    /// returns the structured snapshot instead of a pretty-printed string.
    async fn widget_tree_snapshot(&self) -> holon_pbt_core::capabilities::WidgetSnapshot {
        let empty = || holon_pbt_core::capabilities::WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return empty();
        };
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.render_builder_services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        view_model_to_snapshot(&vm)
    }

    /// Readiness signal for the root render — faithful port of the inline
    /// `inv-viewmodel-snapshot` block's skip guards. Returns `false` (→ the
    /// body must skip its structural assertions) when:
    /// - the root layout isn't watchable yet (`resolve_watch` → `None`, i.e.
    ///   the inline's closed-stream / still-`loading` skip),
    /// - the settled render_expr is the `loading` placeholder,
    /// - the settled render_expr is the `spacer` placeholder, or
    /// - headless interpretation of the render_expr panics (the inline's
    ///   `catch_unwind` skip for the pre-existing shadow-interpret bug).
    ///
    /// Returns `true` only for a settled, interpretable content render —
    /// exactly the state in which the inline ran sub-checks 10a–10j. Roots
    /// at `root_layout_block_uri()`, the same node `widget_tree_snapshot`
    /// roots at and the same node the inline watched (`root_id`).
    async fn root_render_ready(&self) -> bool {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return false;
        };
        let (render_expr, data_rows) = rqr.snapshot();

        let placeholder = matches!(
            &render_expr,
            holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer"
        );
        if placeholder {
            return false;
        }

        let services = self.render_builder_services();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        }))
        .is_ok()
    }

    /// Widget tree for an explicit block id. Builds the snapshot via
    /// interpret_pure against that block's resolved render_expr +
    /// data_rows, same path as `widget_tree_snapshot` but rooted at
    /// `block_id` instead of the layout root. Returns `None` if the
    /// block isn't watchable yet.
    async fn widget_tree_for(
        &self,
        block_id: &EntityUri,
    ) -> Option<holon_pbt_core::capabilities::WidgetSnapshot> {
        let rqr = self.resolve_watch(block_id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.render_builder_services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }

    /// Extracts the `id` column from the layout root's data_rows.
    /// Returns empty set if the layout root isn't watchable yet.
    async fn root_data_row_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return Default::default();
        };
        let (_, data_rows) = rqr.snapshot();
        data_rows
            .iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| EntityUri::parse(s).expect("data_row id must be a valid EntityUri"))
            })
            .collect()
    }

    /// "Decompiler" content comparison for the root layout. Faithful port of
    /// `inv-viewmodel-decompiled-rows-match-query`: interprets the root render_expr against its data_rows into a display
    /// tree, decompiles per-row rendered content via
    /// `extract_rendered_rows`, and pairs the rendered `content` strings with
    /// the query `data_rows`' `content` (filtered to `visible_columns`).
    ///
    /// Returns `None` (→ body `Ok`) when the root isn't watchable / still
    /// loading / a spacer, or when any of the inline gates is empty
    /// (`rendered_rows`, `visible_columns`, `data_rows`).
    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        let (render_expr, data_rows) = rqr.snapshot();

        let services = self.render_builder_services();
        let display_tree =
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();

        let rendered_rows = crate::display_assertions::extract_rendered_rows(&display_tree);

        if std::env::var("HOLON_PBT_ROOT_WATCH_DEBUG").is_ok() {
            eprintln!(
                "[root-watch-debug] render_expr={render_expr:?}\n\
                 [root-watch-debug] data_rows={data_rows:#?}\n\
                 [root-watch-debug] rendered_rows={rendered_rows:#?}"
            );
        }

        // Inline gate: only compare when all three are non-empty.
        if rendered_rows.is_empty() || visible_columns.is_empty() || data_rows.is_empty() {
            return None;
        }

        // Expected: data_rows filtered to visible columns, then the `content`
        // column. Faithful to the inline (filter → extract `content`).
        let data_content: Vec<String> = data_rows
            .iter()
            .map(|r| {
                r.iter()
                    .filter(|(k, _)| visible_columns.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<std::collections::HashMap<String, holon_api::Value>>()
            })
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .collect();

        // Rendered: pull the `content` column directly off each decompiled row
        // (no visible-cols filter — matches the inline).
        let rendered_content: Vec<String> = rendered_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            })
            .collect();

        Some((rendered_content, data_content))
    }
}

/// Frontend-agnostic ViewModel→WidgetSnapshot translator.
///
/// Encoding contract (see `WidgetSnapshot` rustdoc in pbt-core):
/// - `kind` = `vm.widget_name()` or `"unknown"` for un-tagged kinds.
/// - `entity_id` = `vm.row_id()` if present.
/// - `props` = kind-specific scalar fields canonicalised to strings
///   (StateToggle.field/current/label/states, EditableText.field/content, etc.).
/// - `operations` = canonical form `<op_name>:<affected_fields_csv>:<param_names_csv>`,
///   one per `OperationWiring`. Invariants match by prefix
///   (`set_field:task_state:` etc.).
fn view_model_to_snapshot(
    vm: &holon_frontend::view_model::ViewModel,
) -> holon_pbt_core::capabilities::WidgetSnapshot {
    use holon_frontend::view_model::ViewKind;
    use std::collections::BTreeMap;

    let kind = vm.widget_name().unwrap_or("unknown").to_string();
    // For LiveBlock, the referenced block id lives in `kind.block_id`;
    // `vm.row_id()` returns None. Mirror `ViewModel::collect_ids_recursive`
    // semantics by surfacing block_id as entity_id so cross-slice
    // invariants can walk references without knowing about LiveBlock.
    let entity_id = match &vm.kind {
        ViewKind::LiveBlock { block_id, .. } => Some(block_id.clone()),
        _ => vm.row_id(),
    };

    let mut props: BTreeMap<String, String> = BTreeMap::new();
    match &vm.kind {
        ViewKind::StateToggle {
            field,
            current,
            label,
            states,
        } => {
            props.insert("field".into(), field.clone());
            props.insert("current".into(), current.clone());
            props.insert("label".into(), label.clone());
            props.insert("states".into(), states.clone());
        }
        ViewKind::EditableText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
            // Encode trigger count so `inv-viewmodel-editable-text-triggers`
            // can assert non-empty triggers for editors with bound operations
            // without exposing the InputTrigger type to the cross-slice IR.
            props.insert("trigger_count".into(), vm.triggers.len().to_string());
        }
        ViewKind::RenderedText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::Text { content, .. } => {
            props.insert("content".into(), content.clone());
        }
        ViewKind::Badge { label } => {
            props.insert("label".into(), label.clone());
        }
        _ => {}
    }

    let operations: Vec<String> = vm
        .operations
        .iter()
        .map(|ow| {
            let fields_csv = ow.descriptor.affected_fields.join(",");
            let params_csv = ow
                .descriptor
                .required_params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("{}:{}:{}", ow.descriptor.name, fields_csv, params_csv)
        })
        .collect();

    let children = vm.children().iter().map(view_model_to_snapshot).collect();

    holon_pbt_core::capabilities::WidgetSnapshot {
        kind,
        entity_id,
        props,
        operations,
        children,
    }
}

// ─── SutLayout ────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutLayout for E2ESut {
    /// Snapshot the whole BoundsRegistry into the pbt-core
    /// [`RenderedElement`] mirror. `holon-frontend`-only verdicts are
    /// computed here so the bodies stay pure: `expected_size_violation`
    /// runs `ElementInfo::expected_size.check` against a `ProviderEvalCtx`
    /// built from the full snapshot (so `follows_child` cross-refs resolve),
    /// and `is_error_widget` is the `widget_type == "error"` test.
    /// Empty when no geometry provider is installed (headless variants).
    async fn rendered_elements(&self) -> Vec<RenderedElement> {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return Vec::new();
        };
        let all = geometry.all_elements();
        all.iter()
            .map(|(el_id, info)| {
                let ctx = holon_frontend::geometry::ProviderEvalCtx::from_snapshot(
                    &all,
                    el_id.as_str(),
                    None,
                );
                let expected_size_violation = info
                    .expected_size
                    .check(info.width, info.height, &ctx)
                    .err()
                    .map(|v| v.to_string());
                RenderedElement {
                    el_id: el_id.clone(),
                    widget_type: info.widget_type.to_string(),
                    entity_id: info
                        .entity_id
                        .as_deref()
                        .and_then(|s| EntityUri::parse(s).ok()),
                    displayed_text: info.displayed_text.as_deref().map(str::to_string),
                    x: info.x,
                    y: info.y,
                    width: info.width,
                    height: info.height,
                    has_content: info.has_content,
                    parent_id: info.parent_id.as_deref().map(str::to_string),
                    expected_size_violation,
                    is_error_widget: info.widget_type.as_ref() == "error",
                    focused: info.focused,
                }
            })
            .collect()
    }

    /// Fresh snapshot with a frame pump: an occluded GPUI window commits no
    /// frames on its own (`cx.notify()` schedules nothing while the display
    /// link is paused), so a poll loop reading the committed BoundsRegistry
    /// would observe the same stale frame forever. The scroll RPC's handler
    /// unconditionally calls `window.refresh()`, so a scroll request for an
    /// id that matches no row is a pure frame pump.
    async fn rendered_elements_fresh(&self) -> Vec<RenderedElement> {
        if self.render.frontend_geometry.is_some()
            && let Some(driver) = self.driver.as_ref()
        {
            let pump_uri = holon_api::EntityUri::block("__pbt-frame-pump__");
            if let Err(e) = driver.scroll_to_entity(&pump_uri).await {
                tracing::debug!("rendered_elements_fresh: frame pump failed: {e:#}");
            }
        }
        self.rendered_elements().await
    }

    /// Most recent screenshot's content fraction, read from the signal
    /// screenshot watcher's shared `frontend_visual_state`. `None` when no
    /// watcher is installed or no frame has been analysed. Mirrors the inline
    /// `not-visually-empty` read of `self.render.frontend_visual_state`.
    async fn visual_content_fraction(&self) -> Option<f32> {
        let state = self.render.frontend_visual_state.as_ref()?;
        let analysis = *state.lock().unwrap();
        analysis.map(|a| a.content_fraction)
    }

    /// True if any element carrying `id` as its `entity_id` is currently
    /// in the BoundsRegistry. Mirrors the `lookup_entity` helper used in
    /// `inv-frontend-bounds-rendered` (sut.rs:6093–6105).
    async fn has_registered_bounds(&self, id: &EntityUri) -> bool {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return false;
        };
        geometry
            .element_info(&format!("render-entity-{id}"))
            .or_else(|| geometry.element_info(&format!("live-block-{id}")))
            .or_else(|| geometry.element_info(&format!("selectable-{id}")))
            .or_else(|| geometry.element_info(&format!("editable-text-{id}")))
            .or_else(|| geometry.find_by_entity_id(id.as_str()))
            .is_some()
    }

    /// True if a `draggable` element carrying `id` is in the BoundsRegistry.
    /// Mirrors the `tree_draggable` collection in `inv-editable-text-has-draggable`
    /// (sut.rs:6634–6643): an element whose widget_type == "draggable" and
    /// entity_id == id.
    async fn has_draggable_handle(&self, id: &EntityUri) -> bool {
        let Some(ref geometry) = self.render.frontend_geometry else {
            return false;
        };
        geometry.all_elements().into_iter().any(|(_, info)| {
            info.widget_type.as_ref() == "draggable"
                && info.entity_id.as_deref() == Some(id.as_str())
        })
    }

    /// True if any rendered element has widget_type == "error".
    /// Mirrors `inv-frontend-no-error-widgets` (sut.rs:6050–6063) via the
    /// BoundsRegistry; falls back to the ViewModel tree via `frontend_engine`
    /// (pub) when no geometry provider is installed.
    async fn any_error_widget(&self) -> bool {
        if let Some(ref geometry) = self.render.frontend_geometry {
            return geometry
                .all_elements()
                .into_iter()
                .any(|(_, info)| info.widget_type.as_ref() == "error");
        }
        let Some(engine) = self.render.frontend_engine.clone() else {
            return false;
        };
        let root_uri = holon_api::root_layout_block_uri();
        let vm = engine.snapshot(&root_uri);
        crate::display_assertions::count_error_nodes(&vm) > 0
    }

    /// Delegate to `E2ESut::wait_for_entity_bounds` (sut.rs:613), which
    /// owns the polling loop + scroll-into-view RPC + diagnostic dump.
    async fn wait_for_bounds(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.wait_for_entity_bounds(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_widget_kind` (sut.rs:735).
    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        timeout: Duration,
    ) -> Result<(), String> {
        self.wait_for_widget_kind(id.as_str(), accepted, timeout)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_window_focused_editor` (sut.rs).
    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        timeout: Duration,
    ) -> Result<(), String> {
        self.wait_for_window_focused_editor(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

// ─── SutDriver ────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
/// Editor-text capability: per-keystroke editing through the real driver.
/// These have identical signatures to the (removed) `SutHandle` methods —
/// `SutHandle` now lists `SutEditorMirrorWrite` as a supertrait, so the
/// E2E enum's `S: SutHandle` dispatch still satisfies the `TypeChars` /
/// `DeleteBackward` / `MoveCursor` variants that narrow to
/// `S: SutEditorMirrorWrite`. The same variant structs thus run on any
/// SUT supplying this cap (e.g. the pure editor), not just `E2ESut`.
impl E2ESut {
    /// Editor-keystroke gate: block until the PRE-transition active editor
    /// (the block the ref generated this keystroke against) holds WINDOW
    /// focus. Engine focus moves synchronously but window focus follows a
    /// spawned binding — without this gate a fast keystroke is consumed by
    /// the previously-focused editor (silent content corruption that only
    /// surfaces as inv-blocks-match-ref / inv-displayed-text much later).
    /// Fail loud: pressing keys into the wrong editor is never recoverable.
    async fn wait_for_active_editor_window_focus(&self, verb: &str) {
        let Some(block_id) = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.ui.tab.active_editor.as_ref())
            .map(|e| e.block_id.clone())
        else {
            return;
        };
        let resolved = self.resolve_uri(&block_id);
        self.wait_for_window_focused_editor(resolved.as_str(), Duration::from_secs(2))
            .await
            .unwrap_or_else(|e| panic!("[{verb}] active editor not window-focused: {e:#}"));
    }

    /// Per-keystroke gate for key sequences whose earlier keys can MOVE
    /// focus (backspace at offset 0 joins blocks → focus lands on the merged
    /// block, whose editor mounts on a later frame). Re-reads the engine's
    /// CURRENT `focused_block` on every retry — the engine value itself
    /// updates asynchronously via the dispatch result-hook — and waits until
    /// that block's editor reports window focus in a committed frame. No-op
    /// when no frontend engine/geometry is installed.
    async fn wait_for_current_focused_editor_window_focus(&self, verb: &str) {
        if self.render.frontend_geometry.is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let focused = self
                .ctx
                .reactive_engine
                .as_ref()
                .and_then(|e| e.ui_state().focused_block());
            if let Some(block) = focused.as_ref() {
                if self
                    .wait_for_window_focused_editor(block.as_str(), Duration::from_millis(100))
                    .await
                    .is_ok()
                {
                    return;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "[{verb}] engine-focused block {focused:?} never held window focus \
                     within 2s — keystroke would land in the wrong editor"
                );
            }
        }
    }

    /// The committed-frame `displayed_text` of the engine-focused block's
    /// editor, if any. `None` when headless, unfocused, or not yet mounted.
    fn focused_editor_displayed_text(&self) -> Option<String> {
        let geometry = self.render.frontend_geometry.as_deref()?;
        let focused = self
            .ctx
            .reactive_engine
            .as_ref()?
            .ui_state()
            .focused_block()?;
        geometry.all_elements().into_iter().find_map(|(_, info)| {
            (info.entity_id.as_deref() == Some(focused.as_str())
                && info.widget_type.as_ref() == "editable_text")
                .then_some(info.displayed_text.as_deref().map(str::to_string))
                .flatten()
        })
    }

    /// Confirm an editor keystroke LANDED: wait until the focused editor's
    /// on-screen text differs from `before` (every modeled editor keystroke —
    /// a typed char, a deletion, a join — changes the visible text; the
    /// focused block itself may change across a join, which also reads as a
    /// change here). Without this, a keystroke "consumed" by a zombie editor
    /// (e.g. the just-deleted join source still holding window focus until
    /// unmount) silently vanishes. No-op headless. Fail loud on timeout.
    async fn wait_for_editor_text_change(&self, before: &Option<String>, verb: &str) {
        if self.render.frontend_geometry.is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let now = self.focused_editor_displayed_text();
            if now != *before {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "[{verb}] keystroke never changed the focused editor's on-screen text \
                     within 2s (still {before:?}) — it was consumed by a stale editor or lost"
                );
            }
            let _ = tokio::time::timeout(
                Duration::from_millis(50),
                self.render
                    .frontend_geometry
                    .as_deref()
                    .expect("checked above")
                    .changed(),
            )
            .await;
        }
    }
}

impl SutEditorMirrorWrite for E2ESut {
    async fn apply_type_chars(&mut self, text: &str) {
        tracing::trace!("[apply] TypeChars: {:?}", text);
        self.wait_for_active_editor_window_focus("TypeChars").await;
        for ch in text.chars() {
            let keystroke = ch.to_string();
            let before = self.focused_editor_displayed_text();
            self.driver
                .as_ref()
                .expect("driver not installed")
                .send_raw_keystroke(&keystroke, &[])
                .await
                .expect("TypeChars: send_raw_keystroke failed");
            self.wait_for_editor_text_change(&before, "TypeChars").await;
        }
    }

    async fn apply_delete_backward(&mut self, count: usize) {
        tracing::trace!("[apply] DeleteBackward: count={count}");
        self.wait_for_active_editor_window_focus("DeleteBackward")
            .await;
        for i in 0..count {
            // A backspace at offset 0 JOINS blocks and moves focus to the
            // merged block — later backspaces in the same transition must
            // wait for the merged editor to hold window focus or they land
            // in the stale one. Re-gate per keystroke after the first.
            if i > 0 {
                self.wait_for_current_focused_editor_window_focus("DeleteBackward")
                    .await;
            }
            let before = self.focused_editor_displayed_text();
            self.driver
                .as_ref()
                .expect("driver not installed")
                .send_raw_keystroke("backspace", &[])
                .await
                .expect("DeleteBackward: backspace failed");
            self.wait_for_editor_text_change(&before, "DeleteBackward")
                .await;
        }
    }

    async fn apply_move_cursor(&mut self, byte_position: usize) {
        tracing::trace!("[apply] MoveCursor: byte_position={byte_position}");
        // `byte_position` is a byte offset into the active editor text;
        // each `right` keystroke advances one CHAR. Convert against the
        // pre-transition ref editor text (MoveCursor doesn't change it) —
        // on multi-byte content pressing `right` `byte_position` times
        // overshoots the caret.
        let right_presses = {
            let text = self
                .pre_ref_state
                .as_ref()
                .and_then(|s| s.ui.tab.active_editor.as_ref())
                .map(|e| e.in_memory_content.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "[MoveCursor] no pre-transition active editor text — cannot convert \
                         byte position {byte_position} to keystrokes"
                    )
                });
            assert!(
                text.is_char_boundary(byte_position),
                "[MoveCursor] byte_position {byte_position} is not a char boundary of {text:?}"
            );
            text[..byte_position].chars().count()
        };
        self.wait_for_active_editor_window_focus("MoveCursor").await;
        let driver = self.driver.as_ref().expect("driver not installed");
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("MoveCursor: home failed");
        for _ in 0..right_presses {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("MoveCursor: right failed");
        }
    }
}

/// Editor-mirror read capability. Caret delegates to the installed
/// `UserDriver`'s `editor_cursor_byte` observation verb (headless: the
/// `HeadlessEditorMirror` map; GPUI: unobservable → `Err`, disclosed).
/// Live text reads the block's `MutableText` through the frontend
/// engine's `BuilderServices::editable_text` — the same cell headless
/// keystrokes mutate. Both resolve ref-model synthetic ids first.
impl SutEditorMirrorRead for E2ESut {
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        let driver = self.driver.as_ref().ok_or_else(|| {
            "SutEditorMirrorRead::editor_caret_byte: driver not installed".to_string()
        })?;
        let resolved = self.resolve_uri(block_id);
        driver.editor_cursor_byte(&resolved)
    }

    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String> {
        let engine = self.render.frontend_engine.as_ref().ok_or_else(|| {
            "SutEditorMirrorRead::editor_live_text: no frontend engine (SqlOnly headless)"
                .to_string()
        })?;
        let resolved = self.resolve_uri(block_id);
        let services: &dyn BuilderServices = engine.as_ref();
        services
            .editable_text(&resolved, "content")
            .map(|cell| cell.current())
            .map_err(|e| {
                format!(
                    "SutEditorMirrorRead::editor_live_text: no MutableText for {resolved}: {e:#}"
                )
            })
    }
}

/// Block-tree mutation capability: structural edits driven through the
/// real chord/driver pipeline. These are pure ACTIONS — no `ref_state`
/// parameter. The `ref_state`-dependent post-action work (block-count
/// sync barrier, synthetic-id reconciliation onto `doc_uri_map`) lives in
/// `E2ESut::block_tree_post_action`, called by the harness after
/// `apply_to_sut`. `SutHandle` lists `SutBlockTreeWrite` as a supertrait,
/// so the E2E enum's `S: SutHandle` dispatch still satisfies the
/// SplitBlock / JoinBlock / Indent / Outdent / MoveUp / MoveDown variants
/// that narrow to `S: SutBlockTreeWrite`.
#[allow(async_fn_in_trait)]
impl SutBlockTreeWrite for E2ESut {
    async fn apply_split_block(&mut self, block_id: &EntityUri, position: usize) {
        tracing::trace!("[apply] SplitBlock: block={block_id} position={position}");
        let resolved_id = self.resolve_uri(block_id);

        // Bounds pre-condition with SQL probe diagnostic on failure
        // (more actionable than the bare bounds-timeout error).
        if let Err(e) = self
            .wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            let sql_probe = self.probe_block_sql_state(resolved_id.as_str()).await;
            // Layer attribution: is the row in the main-panel WATCH's data
            // rows (then the watch/matview layer is fine and the drop is the
            // live-tree / paint side), or already absent there (then the drop
            // is the query → watch layer: matview/CDC)?
            let mut watch_diag = String::from("<no reactive engine>");
            if let Some(reactive) = self.render.reactive_engine.borrow().clone() {
                let main_panel_id = holon_api::EntityUri::block("default-main-panel");
                let mp_results = reactive.ensure_watching(&main_panel_id);
                let (_, mp_rows) = mp_results.snapshot();
                let row_ids: Vec<String> = mp_rows
                    .iter()
                    .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                    .collect();
                watch_diag = format!(
                    "main-panel watch rows: {} — target present: {}\nids: {row_ids:?}",
                    row_ids.len(),
                    row_ids.iter().any(|id| id == resolved_id.as_str()),
                );
            }
            let live_diag = match self.render.live_tree.borrow().as_ref() {
                None => "<live tree not initialized>".to_string(),
                Some(lt) => {
                    let ids: Vec<String> = lt
                        .items()
                        .iter()
                        .filter_map(|vm| {
                            vm.data
                                .get_cloned()
                                .get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    format!(
                        "live-tree items: {} — target present: {}\nids: {ids:?}",
                        ids.len(),
                        ids.iter().any(|id| id == resolved_id.as_str()),
                    )
                }
            };
            panic!(
                "[SplitBlock] bounds unavailable for {resolved_id}: {e:#}\n\
                 {watch_diag}\n{live_diag}\n\
                 SQL probe for missing entity:\n{sql_probe}"
            );
        }
        // Children-settled gate. `wait_for_entity_bounds` confirms the target
        // appears *somewhere* in the geometry, but coords resolved against a
        // partial first-render get invalidated by the next CDC batch that
        // adds siblings. Wait until every non-Page child of this block's
        // parent — as the PRE-transition ref-state predicted — has rendered
        // so `require_element_center` returns stable bounds. Uses the
        // pre-state instead of `ref_state` (post-transition) so the
        // predicate matches what the user can see right now.
        let parent_for_settle = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.domain.block_state.blocks.get(block_id))
            .map(|b| b.parent_id.clone());
        if let Some(parent_id) = parent_for_settle {
            self.wait_for_children_settled(&parent_id, Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[SplitBlock] children of parent {parent_id} not settled before click: {e:#}"
                    )
                });
        }
        // Pre-Enter SQL snapshot: log the live content + length so panic-time
        // analysis can distinguish "cursor position drift" from "content
        // diverged before the split" cases. Opt-in (`HOLON_PBT_SPLIT_PROBE=1`)
        // — the probe runs extra SQL and prints multi-line noise on EVERY
        // split; the failure paths above print their probes unconditionally.
        if std::env::var("HOLON_PBT_SPLIT_PROBE").as_deref() == Ok("1") {
            let sql_pre = self.probe_block_sql_state(resolved_id.as_str()).await;
            eprintln!("[SplitBlock-presplit] target={resolved_id} position={position}\n{sql_pre}");
        }
        // `position` is a byte offset; the input pipeline presses `right`
        // once per CHAR. Convert against the PRE-transition ref content (the
        // text the user sees before Enter) — on multi-byte content the two
        // units diverge and pressing `right` `position` times overshoots.
        let right_presses = {
            let pre_content = self
                .pre_ref_state
                .as_ref()
                .and_then(|s| s.domain.block_state.blocks.get(block_id))
                .map(|b| b.content.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "[SplitBlock] no pre-transition ref content for {block_id} — cannot \
                         convert byte position {position} to keystrokes"
                    )
                });
            assert!(
                pre_content.is_char_boundary(position),
                "[SplitBlock] position {position} is not a char boundary of {pre_content:?}"
            );
            pre_content[..position].chars().count()
        };
        // Drive the input pipeline (widget-kind → click → focus → keys →
        // Enter) through the capability-bound free helper.
        crate::pbt::transitions::split_block::apply_split_block_input_pipeline_to_sut(
            self,
            &resolved_id,
            right_presses,
        )
        .await;
    }

    async fn apply_join_block(&mut self, block_id: &EntityUri) {
        tracing::trace!("[apply] JoinBlock: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        let mut extra_params = std::collections::HashMap::new();
        extra_params.insert("position".to_string(), holon_api::Value::Integer(0));
        self.dispatch_block_op_via_chord("join_block", resolved_id.as_str(), extra_params)
            .await;
    }

    async fn apply_indent(&mut self, block_id: &EntityUri) {
        tracing::trace!("[apply] Indent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("indent", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_outdent(&mut self, block_id: &EntityUri) {
        tracing::trace!("[apply] Outdent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("outdent", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_move_up(&mut self, block_id: &EntityUri) {
        tracing::trace!("[apply] MoveUp: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_up", resolved_id.as_str(), Default::default())
            .await;
    }

    async fn apply_move_down(&mut self, block_id: &EntityUri) {
        tracing::trace!("[apply] MoveDown: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_down", resolved_id.as_str(), Default::default())
            .await;
    }
}

impl SutDriver for E2ESut {
    /// Send a raw key chord to the currently focused entity.
    /// Not wired: SutDriver::driver_send_key_chord needs a KeyChord
    /// (not a raw string) and a known focused entity id — both are
    /// context-dependent. The existing `E2ESut::send_key_chord` requires
    /// an entity_id param and a parsed `holon_api::KeyChord`. A thin
    /// bridge would need parsing + focus resolution not yet exposed. Wire
    /// in Phase 7 alongside the SutFocus trait (which will expose
    /// `current_focus` from the reference model).
    async fn driver_send_key_chord(&mut self, _: &str) {
        unimplemented!(
            "SutDriver::driver_send_key_chord on E2ESut: requires a focused entity id \
             and a parsed KeyChord; E2ESut::send_key_chord already provides this with \
             explicit args. Bridge in Phase 7 once SutFocus exposes the current focus id."
        )
    }

    /// Click an entity by id via the installed UserDriver. Defaults to
    /// region "main" — the convenience wrapper around `click_entity` used
    /// by SplitBlock / ClickBlock-style transitions.
    async fn driver_click(&mut self, id: &EntityUri) {
        <Self as SutDriver>::click_entity(self, id, "main")
            .await
            .unwrap_or_else(|e| panic!("SutDriver::driver_click failed for {id}: {e}"));
    }

    /// Region-aware click via the installed UserDriver. Returns the
    /// driver error verbatim so callers attach their own
    /// transition-specific diagnostic.
    async fn click_entity(&mut self, id: &EntityUri, region: &str) -> Result<(), String> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| "SutDriver::click_entity: driver not installed".to_string())?;
        driver
            .click_entity(id, region)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delegate to `E2ESut::wait_for_focus_to_match` (sut.rs:793), which
    /// owns the polling loop + focus-and-render diagnostic dump.
    async fn wait_for_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.wait_for_focus_to_match(id.as_str(), timeout)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Returns the current SQL-side focus block id from the `current_focus`
    /// matview. Not wired via the UserDriver (drivers don't expose a
    /// `current_focus()` verb); instead reads the authoritative SQL view,
    /// matching the prod path used by the `inv-navigation-focus` body.
    /// Returns the Main-region focus id, or `None` when the matview is empty.
    async fn driver_current_focus(&self) -> Option<EntityUri> {
        let rows = self
            .ctx
            .query_sql("SELECT region, block_id FROM current_focus")
            .await
            .expect("SutDriver::driver_current_focus: current_focus query failed");
        rows.into_iter()
            .find(|row| {
                row.get("region")
                    .and_then(|v| v.as_string())
                    .map(|r| r == "main")
                    .unwrap_or(false)
            })
            .and_then(|row| {
                row.get("block_id").and_then(|v| v.as_string()).map(|s| {
                    EntityUri::parse(s).expect("current_focus.block_id must be a valid EntityUri")
                })
            })
    }

    /// Returns the globally focused block id as tracked by the frontend
    /// engine's `focused_block()` field. `NoEngine` in SqlOnly mode
    /// (no `frontend_engine` installed); `Unfocused` when the engine has
    /// no focus — distinct so `inv-focus-matches-ref` fails on lost focus
    /// instead of skipping it as "no engine".
    async fn engine_focused_block(&self) -> EngineFocus {
        match self.render.frontend_engine.as_ref() {
            None => EngineFocus::NoEngine,
            Some(engine) => match engine.focused_block() {
                None => EngineFocus::Unfocused,
                Some(id) => EngineFocus::Focused(id),
            },
        }
    }

    /// Translate a reference-model synthetic block id (e.g. `block:ref-doc-0`)
    /// to the resolved UUID-based id the SUT engine tracks. Delegates to
    /// `E2ESut::resolve_uri`, which consults `doc_uri_map`.
    fn resolve_ref_block_id(&self, id: &EntityUri) -> EntityUri {
        self.resolve_uri(id)
    }

    /// Delegate to the installed UserDriver. Returns the driver error
    /// verbatim so callers attach their own transition-specific
    /// diagnostic.
    async fn send_raw_keystroke(&mut self, key: &str, modifiers: &[&str]) -> Result<(), String> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| "SutDriver::send_raw_keystroke: driver not installed".to_string())?;
        driver
            .send_raw_keystroke(key, modifiers)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

// ─── SutOrgRender ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutOrgRender for E2ESut {
    /// Delegates straight to `TestContext::snapshot_org_render_pairs`
    /// (the same path the `InvOrgRenderFixedPoint` body uses).
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)> {
        let pairs = self
            .ctx
            .snapshot_org_render_pairs()
            .await
            .expect("SutOrgRender::snapshot_org_render_pairs: TestContext call failed");
        pairs
            .into_iter()
            .map(|(path, (disk, rendered))| (path.to_string_lossy().to_string(), disk, rendered))
            .collect()
    }
}

// ─── SutOrgRead ───────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutOrgRead for E2ESut {
    /// Wait for the FileSyncController's background re-render to settle, then
    /// parse every tracked org file on disk. Folds
    /// `wait_for_org_files_stable` + `parse_org_file_blocks(None)` into the
    /// single snapshot the `/org` body reads.
    async fn org_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.ctx
            .wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
        self.ctx
            .parse_org_file_blocks(None)
            .await
            .expect("SutOrgRead::org_block_snapshot: failed to parse org files")
    }
}

// ─── SutQueryCompile ──────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl SutQueryCompile for E2ESut {
    /// Not wired: query compilation (PRQL/GQL) requires access to the
    /// holon query-compiler helpers. The compilation path lives in
    /// `crates/holon/src/` and is not currently exposed via `E2ESut` or
    /// `TestContext`. This trait is bound by GENERATORS (not invariants),
    /// so slices without it simply produce no query-content blocks — no
    /// regression. Wire in Phase 7 when a generator actually needs it.
    async fn compile_query(&self, _: &str, _: &str) -> Result<String, String> {
        unimplemented!(
            "SutQueryCompile::compile_query on E2ESut: query compiler helpers are in \
             crates/holon/src/ and not yet exposed via TestContext. Wire in Phase 7 \
             when a generator needs this path."
        )
    }
}

// ─── SutSpanMetrics (otel-testing only) ───────────────────────────────

#[cfg(feature = "otel-testing")]
#[allow(async_fn_in_trait)]
impl crate::pbt::invariants::bodies::sql_budget::SutSpanMetrics for E2ESut {
    /// Port of the inline `inv-sql-budget` block (sut_check_invariants.rs §13):
    /// snapshot span metrics for the last transition, emit all telemetry
    /// side-effects (summary line, N+1 list, flamegraph, detail, memory
    /// diagnosis), and return the budget pass/fail decision. Error violations
    /// are returned only when `HOLON_PERF_BUDGET` enforcement is on; otherwise
    /// they're logged as `BUDGET OFF` (the inline default-off behaviour).
    async fn sql_budget_report(
        &self,
        ref_state: &super::reference_state::ReferenceState,
    ) -> crate::pbt::invariants::bodies::sql_budget::SqlBudgetReport {
        // All span/RSS state + formatting live in `MetricsSut`; `last_transition`
        // is owned by `E2ESut` (it's not a metric) and passed through.
        self.metrics
            .sql_budget_report(&self.last_transition, ref_state)
    }
}
