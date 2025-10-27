//! Loro's durable-state descriptor (`DurableReplicaState` impl).
//!
//! Owns the Loro-specific disk knowledge — the CRDT storage dir holds the
//! LoroDoc plus the persisted sync-base sidecar — so consumers (the
//! consolidator-epoch guard, the future handover migration) never hard-code it.

use std::path::PathBuf;

use holon_core::replica_state::DurableReplicaState;

pub struct LoroDurableState {
    storage_dir: PathBuf,
    enabled: bool,
}

impl LoroDurableState {
    pub fn new(storage_dir: impl Into<PathBuf>, enabled: bool) -> Self {
        Self {
            storage_dir: storage_dir.into(),
            enabled,
        }
    }
}

impl DurableReplicaState for LoroDurableState {
    /// The CRDT dir is durable state when Loro is enabled (it will be written
    /// this session) OR when leftover state from an earlier epoch exists on
    /// disk — a disabled session must still protect (and, on migrate, wipe)
    /// the previous consolidator's history.
    fn durable_paths(&self) -> Vec<PathBuf> {
        if self.enabled || self.storage_dir.exists() {
            vec![self.storage_dir.clone()]
        } else {
            vec![]
        }
    }
}
