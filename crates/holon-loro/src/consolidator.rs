//! Block consolidator — the single owner of the block sink-write.
//!
//! In the Loro-present session, **Loro is the consolidator**: it owns sibling
//! order and merge (pinned at session start via [`SessionCapabilities`]). The
//! projection ([`crate::loro_sync_controller::LoroProjection`]) computes
//! the diff of the Loro authority against the persisted base and hands the
//! result here as a typed-intent [`ChangeSet`] carrying [`Provenance`]. The
//! consolidator records the intent (op-multiset agreement — the Phase-2
//! equivalence relation) and writes the SQL sink.
//!
//! Routing every block sink-write through this one seam is the Phase-5
//! "writes flow as intent to the consolidator" end-state: the projection no
//! longer calls `execute_batch_with_origin` for blocks directly.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use holon_api::{ChangeSet, EntityName, Provenance, agrees_with_ops};

use crate::capability::{Consolidator, SessionCapabilities};
use crate::event_bus::EventOrigin;
use holon_api::StorageEntity;
use holon_core::OriginTaggedWrites;

/// The block consolidator. Wraps the sink command bus and the pinned session
/// capabilities; owns the op-multiset shadow counters.
pub struct BlockConsolidator {
    command_bus: Arc<dyn OriginTaggedWrites>,
    caps: SessionCapabilities,
    agreements: Arc<AtomicUsize>,
    divergences: Arc<AtomicUsize>,
}

impl BlockConsolidator {
    pub fn new(command_bus: Arc<dyn OriginTaggedWrites>, caps: SessionCapabilities) -> Self {
        Self {
            command_bus,
            caps,
            agreements: Arc::new(AtomicUsize::new(0)),
            divergences: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// `(agreements, divergences)` — the op-multiset shadow counters. The gate
    /// requires `divergences == 0` (every emitted op round-tripped through the
    /// typed intent vocabulary).
    pub fn shadow_counters(&self) -> (usize, usize) {
        (
            self.agreements.load(Ordering::SeqCst),
            self.divergences.load(Ordering::SeqCst),
        )
    }

    /// Who owns order/merge this session (pinned at startup).
    pub fn consolidator(&self) -> Consolidator {
        self.caps.consolidator()
    }

    /// Apply a block change batch as a typed intent.
    ///
    /// `ops` is the lossless diff the projection computed (Loro authority vs the
    /// persisted base); `provenance` records the base it was diffed against (and,
    /// when known, the originating command). The consolidator records the typed
    /// [`ChangeSet`] — asserting it round-trips to the same op multiset (the
    /// intent vocabulary must capture every op) — and writes the SQL sink. This
    /// is the ONLY block sink-write path.
    ///
    /// Divergences are logged loudly and counted but never abort the write: the
    /// write carries the lossless `ops`, and the gate checks the counter.
    pub async fn apply(
        &self,
        ops: Vec<(String, StorageEntity)>,
        provenance: Provenance,
    ) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(
            self.caps.consolidator(),
            Consolidator::Upstream,
            "BlockConsolidator writes the upstream→SQL projection sink; expected an upstream consolidator"
        );

        let change_set = ChangeSet::from_ops(&ops, provenance);
        match agrees_with_ops(&change_set, &ops) {
            Ok(()) => {
                self.agreements.fetch_add(1, Ordering::SeqCst);
                tracing::debug!(
                    "[BlockConsolidator] intent AGREE: {} op(s) → {} typed",
                    ops.len(),
                    change_set.ops.len(),
                );
            }
            Err(why) => {
                self.divergences.fetch_add(1, Ordering::SeqCst);
                tracing::error!("[BlockConsolidator] intent DIVERGENCE: {why}");
            }
        }

        // Loro is the consolidator (owns merge/order); the SQL sink is its
        // derived single-writer projection. Tag the write `EventOrigin::Loro`.
        self.command_bus
            .execute_batch_with_origin(&EntityName::new("block"), ops, EventOrigin::Loro)
            .await
            .map_err(|e| anyhow::anyhow!("BlockConsolidator sink write failed: {}", e))?;
        Ok(())
    }
}
