//! The UI's block read model — published at the CRDT commit, not read back
//! out of SQL.
//!
//! In CRDT mode the consolidator's own diff is the freshest correct statement
//! of the block set: it exists before the SQL sink write, which is what today
//! makes every keystroke wait in the single sequential Turso actor. This port
//! is where that diff is published. The SQL projection remains an index —
//! maintained from the same diff, never the UI's read path (ADR: source-of-
//! truth inversion; Model.md inv 16).
//!
//! Two publishing shapes, and the difference matters:
//!
//! * [`BlockReadModel::publish_delta`] — the deduplicated per-commit delta.
//! * [`BlockReadModel::publish_snapshot`] — a whole-set replacement, delivered
//!   in ONE generation. A producer that re-derives its entire set (the
//!   projection's reseed) must use this; clear-then-insert would show every
//!   downstream group momentarily empty.
//!
//! The item type is [`SnapshotBlock`], not `Block`: `sort_key` is the storage
//! adapter's ordering and rides with the row, so a consumer can order children
//! without a second read.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use crate::block::SnapshotBlock;
use crate::live_data::AuthoredLiveData;
use crate::live_data::LiveData;

/// Read side of the read model — what a consumer (a widget, a view model) is
/// given. A port so `holon-frontend` can read the model while depending only
/// on `holon-api`; `holon-loro` owns the only production producer.
pub trait BlockDeltaSource: Send + Sync {
    /// The live block set, keyed by block id.
    fn blocks(&self) -> Arc<LiveData<SnapshotBlock>>;
}

/// Write side: the producer's handle onto the same mirror.
pub struct BlockReadModel {
    /// The authored handle, not a bare `Arc<LiveData<_>>`: only this type
    /// carries `replace_all`, the atomic re-snapshot `publish_snapshot` needs.
    blocks: AuthoredLiveData<SnapshotBlock>,
}

impl BlockReadModel {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            blocks: LiveData::in_memory(),
        })
    }

    /// Publish one commit's delta. `Some(block)` upserts, `None` retracts.
    ///
    /// The caller passes the ALREADY-DEDUPLICATED delta (the projection's
    /// `staging`, which its compare-and-skip has stripped of no-ops).
    /// Re-publishing a value equal to the one already held is the churn this
    /// model exists to remove, so nothing here re-filters it — a caller that
    /// hands over raw changes is the bug.
    pub fn publish_delta(&self, delta: &[(String, Option<SnapshotBlock>)]) {
        for (id, new) in delta {
            match new {
                Some(block) => self.blocks.insert(id.clone(), Arc::new(block.clone())),
                None => {
                    self.blocks.remove(id);
                }
            }
        }
    }

    /// Replace the whole set atomically — see [`LiveData::replace_all`].
    pub fn publish_snapshot(&self, snapshot: &HashMap<String, SnapshotBlock>) {
        let items: BTreeMap<String, Arc<SnapshotBlock>> = snapshot
            .iter()
            .map(|(id, block)| (id.clone(), Arc::new(block.clone())))
            .collect();
        self.blocks.replace_all(items);
    }
}

impl BlockDeltaSource for BlockReadModel {
    fn blocks(&self) -> Arc<LiveData<SnapshotBlock>> {
        self.blocks.shared()
    }
}
