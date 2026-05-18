//! Blanket `SutLoro` + `SutLoroLog` + `SutLifecycle` impls on `E2ESut<V>`.
//!
//! Follows the same pattern as `reference_capabilities.rs`:
//! thin forwarding impls that expose capability-trait surface
//! over existing inherent / `SutHandle` methods.

use std::collections::{BTreeSet, HashSet};

use holon_pbt_core::capabilities::{
    CapBlockId, SutCdc, SutLifecycle, SutLoro, SutLoroLog, SutOrgFileWrite, SutSqlProjection,
};

use super::sut::E2ESut;
use super::transition_dispatch::SutHandle;
use super::transitions::{PeerEditOp, TextOp};
use super::types::VariantMarker;

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLoro for E2ESut<V> {
    async fn apply_add_peer(&mut self) {
        <E2ESut<V> as SutHandle>::apply_add_peer(self).await;
    }

    async fn apply_peer_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Create {
                parent_stable_id: parent_stable_id.map(str::to_owned),
                content: content.to_owned(),
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Update {
                stable_id: stable_id.to_owned(),
                content: content.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_delete(&mut self, peer_idx: usize, stable_id: &str) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Delete {
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_char_edit(
            self,
            peer_idx,
            stable_id,
            &TextOp::Insert {
                pos_codepoint,
                text: text.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_char_edit(
            self,
            peer_idx,
            stable_id,
            &TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            },
        )
        .await;
    }

    async fn apply_sync_with_peer(&mut self, peer_idx: usize) {
        <E2ESut<V> as SutHandle>::apply_sync_with_peer(self, peer_idx).await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        <E2ESut<V> as SutHandle>::apply_merge_from_peer(self, peer_idx).await;
    }

    /// Not wired: the capability trait models lag-based stale peers
    /// (`lag_steps: usize`) but `E2ESut::apply_create_stale_loro` takes
    /// `(org_filename, LoroCorruptionType)` — a pre-startup file-corruption
    /// concept. Phase 7 will decide whether to reconcile the two models or
    /// keep them separate. Until then this panics loudly if called.
    async fn apply_create_stale_loro(&mut self, _: usize) {
        unimplemented!(
            "SutLoro::apply_create_stale_loro on E2ESut: lag_steps-based peer snapshots \
             are not wired yet. The underlying SUT method takes (org_filename, \
             LoroCorruptionType) — a different model. Wire in Phase 7 once the \
             semantics are reconciled."
        )
    }
}

// ─── SutLifecycle ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLifecycle for E2ESut<V> {
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
impl<V: VariantMarker> SutLoroLog for E2ESut<V> {
    /// Delegates to `TestContext::loro_sync_error_count`, which reads the
    /// `LoroSyncController`'s atomic error counter. Same source as the
    /// `inv-loro-no-errors` check in `check_invariants_async`.
    async fn loro_had_errors(&self) -> bool {
        self.ctx.loro_sync_error_count() > 0
    }

    /// Not wired: requires access to the LoroSyncController's internal tree
    /// snapshot, which today is only read inside `check_invariants_async` via
    /// the controller's `tree_children` helper. Phase 7 will expose this
    /// through `TestContext` once `inv-live-children-match-ref` migrates onto
    /// `SutLoroLog`.
    async fn loro_children_of(&self, _: &str) -> Option<Vec<String>> {
        unimplemented!(
            "SutLoroLog::loro_children_of on E2ESut: Loro tree child snapshot is not yet \
             exposed through TestContext. Wire in Phase 7 alongside the \
             inv-live-children-match-ref invariant migration."
        )
    }
}

// ─── SutSqlProjection ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutSqlProjection for E2ESut<V> {
    /// Queries the `block` materialized view for a single row and returns its
    /// fields as strings. Same data path as `check_invariants_async`'s
    /// `inv-backend-blocks-match-ref` block matview read (sut.rs:4807).
    async fn block_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        let escaped = id.replace('\'', "''");
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
    /// `SELECT id FROM block_raw` query in `check_invariants_async`
    /// (sut.rs:4228).
    async fn all_block_ids(&self) -> BTreeSet<CapBlockId> {
        let rows = self
            .ctx
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("SutSqlProjection::all_block_ids query failed");
        rows.into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
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
    /// single row. Used by the WARN/SKIP CDC-lag classifier in
    /// `check_invariants_async` (sut.rs:3189–3206 pattern).
    async fn block_raw_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        let escaped = id.replace('\'', "''");
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
}

// ─── SutOrgFileWrite ──────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutOrgFileWrite for E2ESut<V> {
    /// Delegates to `SutHandle::apply_write_org_file`, which writes the file
    /// via `TestContext::write_org_file` and — when the app is running — waits
    /// for `OrgSyncController` to ingest it and re-key `ctx.documents`.
    async fn write_org_file(&mut self, path: &str, contents: &str) {
        <E2ESut<V> as SutHandle>::apply_write_org_file(self, path, contents).await;
    }
}

// ─── SutCdc ───────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutCdc for E2ESut<V> {
    /// Not wired: the production stale-check (`live_blocks_stale` in
    /// `check_invariants_async`, sut.rs:4225) relies on `live_blocks_cell`
    /// which is a private field of `E2ESut`. The `LiveData::consumed_seq()`
    /// vs `db_handle().cdc_emitted_watermark()` comparison needs either a
    /// pub(super) accessor added to `E2ESut` or a dedicated helper on
    /// `TestContext`. Wire in Phase 7 when the first slice-level invariant
    /// needs this signal.
    async fn cdc_in_flight(&self) -> bool {
        unimplemented!(
            "SutCdc::cdc_in_flight on E2ESut: requires comparing \
             live_blocks_cell.consumed_seq() against db_handle().cdc_emitted_watermark(), \
             but live_blocks_cell is a private field. Add a pub(super) accessor to E2ESut \
             or a TestContext helper and wire in Phase 7."
        )
    }

    /// Drains pending CDC events from all active watches into the `ui_model`.
    /// Delegates to `TestContext::drain_cdc_events` — same logic used between
    /// transitions in `check_invariants_async` (sut.rs:3965, 4005).
    async fn drain_cdc(&mut self) {
        self.ctx.drain_cdc_events().await;
    }
}
