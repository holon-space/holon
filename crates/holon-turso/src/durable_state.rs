//! Turso's durable-state descriptor (`DurableReplicaState` impl).
//!
//! Owns the Turso-specific disk knowledge — `:memory:` detection and the
//! `-wal`/`-shm` sidecar files — so consumers (the consolidator-epoch guard,
//! the future handover migration) never hard-code it.

use holon_core::replica_state::DurableReplicaState;
use std::path::{Path, PathBuf};

pub struct TursoDurableState {
    db_path: PathBuf,
}

impl TursoDurableState {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    fn is_ephemeral(&self) -> bool {
        let s = self.db_path.to_string_lossy();
        s.is_empty() || s.starts_with(":memory:")
    }
}

impl DurableReplicaState for TursoDurableState {
    fn durable_paths(&self) -> Vec<PathBuf> {
        if self.is_ephemeral() {
            return vec![];
        }
        ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| {
                if suffix.is_empty() {
                    self.db_path.clone()
                } else {
                    PathBuf::from(format!("{}{}", self.db_path.display(), suffix))
                }
            })
            .collect()
    }
}

/// Where per-instance Holon metadata (e.g. the consolidator-epoch marker)
/// can live so it sits next to the durable db but survives a db wipe.
/// `None` when the db is ephemeral — there is no durable state to anchor to.
pub fn instance_state_root(db_path: &Path) -> Option<PathBuf> {
    let state = TursoDurableState::new(db_path);
    if state.is_ephemeral() {
        return None;
    }
    let parent = db_path.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.join(".holon"))
}
