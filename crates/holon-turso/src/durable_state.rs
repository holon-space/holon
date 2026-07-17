//! Turso's durable-state descriptor (`DurableReplicaState` impl).
//!
//! Owns the Turso-specific disk knowledge — `:memory:` detection and the
//! `-wal`/`-shm` sidecar files — so consumers (the consolidator-epoch guard,
//! the future handover migration) never hard-code it.

use std::path::Path;
use std::path::PathBuf;

use holon_core::replica_state::DurableReplicaState;

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

#[cfg(test)]
mod tests {
    use super::*;

    // is_ephemeral is reached through the public DurableReplicaState surface;
    // these pin both truth values and the `||` vs `&&` shape (invariant 10:
    // the epoch guard uses durable_paths to decide what a mode-switch may wipe).

    #[test]
    fn file_backed_db_is_durable_with_all_sidecars() {
        let state = TursoDurableState::new("/var/data/holon.db");
        // file-backed => is_ephemeral() == false => the three sidecar paths.
        assert_eq!(
            state.durable_paths(),
            vec![
                PathBuf::from("/var/data/holon.db"),
                PathBuf::from("/var/data/holon.db-wal"),
                PathBuf::from("/var/data/holon.db-shm"),
            ]
        );
    }

    #[test]
    fn memory_db_is_ephemeral_no_paths() {
        // starts_with(":memory:") arm of the `||`.
        assert!(
            TursoDurableState::new(":memory:")
                .durable_paths()
                .is_empty()
        );
    }

    #[test]
    fn empty_path_is_ephemeral_no_paths() {
        // is_empty() arm of the `||` — kills `|| -> &&` (empty && :memory: is
        // never both-true, so `&&` would wrongly report empty as durable).
        assert!(TursoDurableState::new("").durable_paths().is_empty());
    }

    #[test]
    fn instance_state_root_file_backed_anchors_next_to_db() {
        assert_eq!(
            instance_state_root(Path::new("/var/data/holon.db")),
            Some(PathBuf::from("/var/data/.holon"))
        );
    }

    #[test]
    fn instance_state_root_memory_is_none() {
        assert_eq!(instance_state_root(Path::new(":memory:")), None);
    }

    #[test]
    fn instance_state_root_bare_filename_has_no_parent_dir() {
        // parent() is Some("") for a bare name => the empty-parent guard => None.
        assert_eq!(instance_state_root(Path::new("holon.db")), None);
    }
}
