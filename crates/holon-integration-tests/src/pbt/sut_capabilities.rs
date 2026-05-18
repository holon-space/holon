//! Blanket `SutLoro` + `SutLoroLog` + `SutLifecycle` impls on `E2ESut<V>`.
//!
//! Follows the same pattern as `reference_capabilities.rs`:
//! thin forwarding impls that expose capability-trait surface
//! over existing inherent / `SutHandle` methods.

use std::collections::HashSet;

use holon_pbt_core::capabilities::{SutLifecycle, SutLoro, SutLoroLog};

use super::sut::E2ESut;
use super::transition_dispatch::SutHandle;
use super::transitions::{PeerEditOp, TextOp};
use super::types::VariantMarker;

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLoro for E2ESut<V> {
    async fn apply_add_peer(&mut self) {
        SutHandle::apply_add_peer(self).await;
    }

    async fn apply_peer_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        SutHandle::apply_peer_edit(
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
        SutHandle::apply_peer_edit(
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
        SutHandle::apply_peer_edit(
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
        SutHandle::apply_peer_char_edit(
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
        SutHandle::apply_peer_char_edit(
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
        SutHandle::apply_sync_with_peer(self, peer_idx).await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        SutHandle::apply_merge_from_peer(self, peer_idx).await;
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
