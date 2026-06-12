//! The SUT component of the Loro-storage slice: a [`CapProvider`] wrapping a
//! **real** `holon-loro` `LoroBackend` (the production CRDT, not a re-impl).
//!
//! It provides two caps:
//! - [`SutBackend`] over the Loro store — so **every already-wired block-tree
//!   invariant** in the shared catalog (`no_parent_cycles`, `source_language`,
//!   `blocks_match`, `no_orphan`, `block_content`, `block_parent`) runs over the
//!   Loro realization *for free*, the §6 payoff: a new component lights up the
//!   whole catalog without touching it.
//! - [`SutLoroLog`] — the Loro-specific read surface (`loro_children_of` exposes
//!   the tree's authoritative fractional-index sibling order) that unlocks the
//!   `inv-loro-children-match-ref` cross-check against the independent reference.

use std::sync::Arc;

use holon::api::LoroBackend;
use holon::api::types::Traversal;
use holon_api::repository::CoreOperations;
use holon_pbt_core::capabilities::{SutBackend, SutLoroLog, SutLoroTaskState};
use holon_pbt_core::composition::{CapMap, CapProvider};

/// A composition component wrapping a real `LoroBackend` CRDT. Reads the live
/// Loro tree directly through `CoreOperations` — there is no SQL projection and
/// no CDC mirror, so (as with the memory slice) the "live" view *is* the store.
///
/// Holds the backend behind `Arc` so a write target can commit into the same
/// store this read cap observes (the F2 committed-content keystone). `Arc` keeps
/// the sharing uniform with the memory slice; `LoroBackend::clone` already shares
/// the CRDT doc, but `Arc` makes the single-store contract explicit.
pub struct LoroBackendComponent {
    backend: Arc<LoroBackend>,
}

impl LoroBackendComponent {
    pub fn new(backend: LoroBackend) -> Self {
        Self::new_shared(Arc::new(backend))
    }

    /// Wrap an already-shared backend so the read cap and a write target observe
    /// one store.
    pub fn new_shared(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }

    async fn all_blocks(&self) -> Vec<holon_api::Block> {
        self.backend
            .get_all_blocks(Traversal::ALL)
            .await
            .expect("LoroBackend::get_all_blocks must not fail in-memory")
    }
}

#[async_trait::async_trait(?Send)]
impl SutBackend for LoroBackendComponent {
    /// No CDC matview — the live view is the Loro tree itself (convergent by
    /// construction), so `live` and `block_raw` snapshots read the same truth.
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.all_blocks().await
    }

    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block> {
        self.all_blocks().await
    }

    /// The Loro slice has no focus-roots projection. No cap-selected invariant
    /// reads it (selection guarantees this), so an empty mirror is the honest
    /// answer rather than a fabricated row.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroLog for LoroBackendComponent {
    /// The standalone CRDT has no `LoroSyncController` — that error counter
    /// belongs to the SQL→Loro mirror, which is absent in this pure-Loro slice.
    /// So there is no error source and the honest answer is `false` (§5.1 hidden
    /// capability: an honest empty/zero, never a fabricated value). The teeth of
    /// `inv-loro-no-errors` live in its fixture-driven catch test, and in the
    /// E2E slice where a real controller wires its counter.
    async fn loro_had_errors(&self) -> bool {
        false
    }

    /// Ordered child block ids of `parent` in the live Loro tree, in the tree's
    /// internal **fractional-index** order (the authoritative sibling order) —
    /// exactly what `list_children` returns. `None` when `parent` isn't a block
    /// in the tree (e.g. the virtual root): the body then skips it, so root-level
    /// order is not asserted here, matching the E2ESut realization.
    async fn loro_children_of(&self, parent: &str) -> Option<Vec<String>> {
        let all = self.all_blocks().await;
        all.iter().find(|b| b.id.as_str() == parent)?;
        Some(
            self.backend
                .list_children(parent)
                .await
                .expect("LoroBackend::list_children must not fail for a present parent"),
        )
    }

    /// Every block held in the live Loro tree, typed. Always `Some` here (Loro
    /// is, by definition, enabled on this component).
    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>> {
        Some(self.all_blocks().await)
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroTaskState for LoroBackendComponent {
    /// Project a block's `task_state` from the live Loro tree — the
    /// `properties["task_state"]` scalar, exactly the value the SQL sibling
    /// [`holon_pbt_core::capabilities::SutSqlProjection::block_task_state`]
    /// reads via `json_extract`. `None` when the block is absent or carries no
    /// `task_state`, so `inv-task-state-storage-coherence` compares like for
    /// like across the two independent storage realizations.
    async fn loro_task_state_of(&self, block_id: &str) -> Option<String> {
        let blocks = self.all_blocks().await;
        let block = blocks.iter().find(|b| b.id.as_str() == block_id)?;
        block
            .properties
            .get("task_state")
            .and_then(|v| v.as_string_owned())
    }
}

impl CapProvider for LoroBackendComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutBackend>);
        caps.insert(self.clone() as Arc<dyn SutLoroLog>);
        caps.insert(self as Arc<dyn SutLoroTaskState>);
    }
}
