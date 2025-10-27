//! On-disk persistence for per-share `LoroDoc` snapshots.
//!
//! Parallel to [`crate::sync::loro_document_store::LoroDocumentStore`],
//! but keyed by `shared_tree_id` and holding many files under a
//! `shares/` subdirectory of the storage root.
//!
//! A shared subtree has been pruned from the global `holon_tree.loro`
//! at share time — the per-share snapshot is **the only copy** of its
//! content on this device. Two behaviours follow:
//!
//! - **Atomic save**: write to `<id>.loro.tmp`, `fsync`, rename to
//!   `<id>.loro`. A torn write leaves the previous snapshot intact.
//! - **Quarantine on corrupt**: if `LoroDoc::import` fails we move the
//!   file to `<id>.loro.corrupt-<rfc3339-ts>` and emit
//!   [`ShareDegraded::SnapshotLoadFailed`], rather than deleting.
//!   The `LoroDocumentStore` "delete on decode error" pattern would
//!   be unrecoverable data loss here.
//!
//! `sweep_stale_tmps` runs once at startup to clean leftover `.tmp`
//! files from crashed previous writes.

use crate::sync::degraded_signal_bus::{DegradedSignalBus, ShareDegraded, ShareDegradedReason};
use anyhow::{Context, Result, anyhow};
use iroh::EndpointAddr;
use loro::{ExportMode, LoroDoc};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, warn};

pub struct SharedSnapshotStore {
    shares_dir: PathBuf,
    bus: Arc<DegradedSignalBus>,
    /// Test-only counter so `test_save_worker_burst_coalescing` can
    /// observe how many times the file was actually written.
    #[cfg(test)]
    write_counter: std::sync::atomic::AtomicUsize,
}

impl SharedSnapshotStore {
    /// `base_storage_dir` is the Loro storage root (same dir that
    /// holds `holon_tree.loro` and `device.key`). Snapshots live under
    /// `<base_storage_dir>/shares/`.
    pub fn new(base_storage_dir: PathBuf, bus: Arc<DegradedSignalBus>) -> Self {
        Self {
            shares_dir: base_storage_dir.join("shares"),
            bus,
            #[cfg(test)]
            write_counter: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn shares_dir(&self) -> &Path {
        &self.shares_dir
    }

    pub fn snapshot_path(&self, shared_tree_id: &str) -> PathBuf {
        self.shares_dir.join(format!("{shared_tree_id}.loro"))
    }

    fn tmp_path(&self, shared_tree_id: &str) -> PathBuf {
        self.shares_dir.join(format!("{shared_tree_id}.loro.tmp"))
    }

    /// Sidecar path for persisted known-peer addresses. Sits next to
    /// the snapshot so `ls shares/` shows them paired. JSON — peer
    /// addresses are per-device local state, intentionally not
    /// replicated through the shared CRDT.
    pub fn peers_path(&self, shared_tree_id: &str) -> PathBuf {
        self.shares_dir.join(format!("{shared_tree_id}.peers.json"))
    }

    fn peers_tmp_path(&self, shared_tree_id: &str) -> PathBuf {
        self.shares_dir
            .join(format!("{shared_tree_id}.peers.json.tmp"))
    }

    /// Atomic write: tmp → fsync → rename. Overwrites any existing
    /// snapshot safely.
    pub fn save(&self, shared_tree_id: &str, doc: &LoroDoc) -> Result<()> {
        let bytes = doc
            .export(ExportMode::Snapshot)
            .context("export shared doc snapshot")?;
        let final_path = self.snapshot_path(shared_tree_id);
        let tmp_path = self.tmp_path(shared_tree_id);

        std::fs::create_dir_all(&self.shares_dir)
            .with_context(|| format!("create {}", self.shares_dir.display()))?;

        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("create tmp {}", tmp_path.display()))?;
            f.write_all(&bytes)
                .with_context(|| format!("write tmp {}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync tmp {}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, &final_path)
            .with_context(|| format!("rename {} → {}", tmp_path.display(), final_path.display()))?;
        if let Ok(dir) = std::fs::File::open(&self.shares_dir) {
            let _ = dir.sync_all();
        }

        #[cfg(test)]
        self.write_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Ok(())
    }

    /// Atomically write the known-peer list for `shared_tree_id`. The
    /// sidecar is a JSON array of `EndpointAddr`s — iroh derives
    /// `Serialize`/`Deserialize` on both `EndpointAddr` and the
    /// underlying `TransportAddr` variants, so no manual schema needed.
    /// Overwrites any previous sidecar atomically.
    pub fn save_peers(&self, shared_tree_id: &str, peers: &[EndpointAddr]) -> Result<()> {
        let bytes = serde_json::to_vec(peers).context("serialize known peers as JSON")?;
        let final_path = self.peers_path(shared_tree_id);
        let tmp_path = self.peers_tmp_path(shared_tree_id);

        std::fs::create_dir_all(&self.shares_dir)
            .with_context(|| format!("create {}", self.shares_dir.display()))?;

        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("create tmp {}", tmp_path.display()))?;
            f.write_all(&bytes)
                .with_context(|| format!("write tmp {}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync tmp {}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, &final_path)
            .with_context(|| format!("rename {} → {}", tmp_path.display(), final_path.display()))?;
        if let Ok(dir) = std::fs::File::open(&self.shares_dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Load the sidecar peer list. Missing file → empty vec (fresh
    /// share, pre-restart, or never persisted — all benign). Malformed
    /// JSON is an error so the caller can decide between surfacing it
    /// and starting over with an empty peer set.
    pub fn load_peers(&self, shared_tree_id: &str) -> Result<Vec<EndpointAddr>> {
        let path = self.peers_path(shared_tree_id);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let peers: Vec<EndpointAddr> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse peer sidecar {}", path.display()))?;
        Ok(peers)
    }

    /// Load `<id>.loro`. On decode error, quarantine the file (rename
    /// to `<id>.loro.corrupt-<rfc3339-ts>`) and emit
    /// [`ShareDegraded::SnapshotLoadFailed`] on the bus. Returns `Err`
    /// — the caller skips this share.
    pub fn load(&self, shared_tree_id: &str) -> Result<LoroDoc> {
        let path = self.snapshot_path(shared_tree_id);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        // Configure mark styles before any import so subsequent mark
        // applications honor the per-key `ExpandType` policy. Without
        // this, `LoroText::mark` silently no-ops and reads return empty
        // mark sets — see Phase 0.1 spike S3.
        let doc = LoroDoc::new();
        crate::api::loro_backend::configure_text_styles(&doc);
        match doc.import(&bytes) {
            Ok(_) => Ok(doc),
            Err(e) => {
                let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
                let quarantine = self
                    .shares_dir
                    .join(format!("{shared_tree_id}.loro.corrupt-{ts}"));
                match std::fs::rename(&path, &quarantine) {
                    Ok(_) => warn!(
                        "[snapshot_store] corrupt snapshot {} quarantined at {}: {e}",
                        path.display(),
                        quarantine.display()
                    ),
                    Err(rename_err) => warn!(
                        "[snapshot_store] corrupt snapshot {} could not be renamed to {}: {rename_err}; decode error was: {e}",
                        path.display(),
                        quarantine.display()
                    ),
                }
                self.bus.emit(ShareDegraded {
                    shared_tree_id: shared_tree_id.to_string(),
                    reason: ShareDegradedReason::SnapshotLoadFailed(
                        quarantine.display().to_string(),
                    ),
                });
                Err(anyhow!(
                    "shared snapshot {shared_tree_id} is corrupt; quarantined at {}: {e}",
                    quarantine.display()
                ))
            }
        }
    }

    /// Does `<id>.loro` exist on disk?
    pub fn exists(&self, shared_tree_id: &str) -> bool {
        self.snapshot_path(shared_tree_id).is_file()
    }

    /// Remove any `*.loro.tmp` files left by a crashed previous write.
    /// Returns the count of files removed. Called once at startup.
    pub fn sweep_stale_tmps(&self) -> Result<usize> {
        if !self.shares_dir.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.shares_dir)
            .with_context(|| format!("read_dir {}", self.shares_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".loro.tmp") || n.ends_with(".peers.json.tmp"))
                .unwrap_or(false)
            {
                match std::fs::remove_file(&path) {
                    Ok(_) => {
                        removed += 1;
                        debug!("[snapshot_store] removed stale tmp file {}", path.display());
                    }
                    Err(e) => warn!(
                        "[snapshot_store] failed to remove stale tmp {}: {e}",
                        path.display()
                    ),
                }
            }
        }
        Ok(removed)
    }

    /// Delete the snapshot file, its peers sidecar, and any
    /// `<id>.loro.corrupt-*` quarantine siblings for `shared_tree_id`.
    /// Returns the number of files removed. Missing files are NOT an
    /// error — this op is expected to run on orphaned ids where some
    /// artifacts may already be absent.
    pub fn delete_snapshot(&self, shared_tree_id: &str) -> Result<usize> {
        let mut removed = 0;
        if !self.shares_dir.exists() {
            return Ok(0);
        }
        let prefix_corrupt = format!("{shared_tree_id}.loro.corrupt-");
        for entry in std::fs::read_dir(&self.shares_dir)
            .with_context(|| format!("read_dir {}", self.shares_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let matches = name == format!("{shared_tree_id}.loro")
                || name == format!("{shared_tree_id}.peers.json")
                || name.starts_with(&prefix_corrupt);
            if matches {
                std::fs::remove_file(&path)
                    .with_context(|| format!("remove {}", path.display()))?;
                removed += 1;
            }
        }
        if let Ok(dir) = std::fs::File::open(&self.shares_dir) {
            let _ = dir.sync_all();
        }
        Ok(removed)
    }

    /// List `shared_tree_id`s for every `<id>.loro` currently on disk.
    /// Diagnostic only — rehydration iterates mount nodes in the
    /// global tree, not the filesystem.
    pub fn list_snapshots(&self) -> Result<Vec<String>> {
        if !self.shares_dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.shares_dir)
            .with_context(|| format!("read_dir {}", self.shares_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(id) = name.strip_suffix(".loro") {
                    ids.push(id.to_string());
                }
            }
        }
        Ok(ids)
    }

    #[cfg(test)]
    pub fn write_count(&self) -> usize {
        self.write_counter
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store_in(dir: &Path) -> (SharedSnapshotStore, Arc<DegradedSignalBus>) {
        let bus = Arc::new(DegradedSignalBus::new());
        let store = SharedSnapshotStore::new(dir.to_path_buf(), bus.clone());
        (store, bus)
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let (store, _bus) = store_in(dir.path());

        let doc = LoroDoc::new();
        let tree = doc.get_tree("tree");
        let node = tree.create(None::<loro::TreeID>).unwrap();
        tree.get_meta(node).unwrap().insert("k", "v").unwrap();
        doc.commit();

        store.save("abc", &doc).unwrap();
        assert!(store.exists("abc"));
        assert!(dir.path().join("shares").join("abc.loro").exists());

        let reloaded = store.load("abc").unwrap();
        let reloaded_tree = reloaded.get_tree("tree");
        let roots = reloaded_tree.roots();
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn save_does_not_leave_tmp_file() {
        let dir = TempDir::new().unwrap();
        let (store, _bus) = store_in(dir.path());
        let doc = LoroDoc::new();
        store.save("x", &doc).unwrap();
        let tmps: Vec<_> = std::fs::read_dir(dir.path().join("shares"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".tmp"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(tmps.is_empty(), "unexpected .tmp files: {tmps:?}");
    }

    #[test]
    fn load_corrupt_file_quarantines_and_emits() {
        let dir = TempDir::new().unwrap();
        let (store, bus) = store_in(dir.path());
        std::fs::create_dir_all(dir.path().join("shares")).unwrap();
        std::fs::write(
            dir.path().join("shares").join("bad.loro"),
            b"not a loro doc",
        )
        .unwrap();

        let mut rx = bus.subscribe();
        let err = store.load("bad").unwrap_err();
        assert!(format!("{err}").contains("corrupt"));
        // Original file gone.
        assert!(!dir.path().join("shares").join("bad.loro").exists());
        // A quarantine file exists.
        let quarantined: Vec<_> = std::fs::read_dir(dir.path().join("shares"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("bad.loro.corrupt-"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(quarantined.len(), 1, "expected exactly one quarantine file");
        // Bus got a SnapshotLoadFailed event.
        let ev = rx.try_recv().expect("no event on bus");
        assert_eq!(ev.shared_tree_id, "bad");
        assert!(matches!(
            ev.reason,
            ShareDegradedReason::SnapshotLoadFailed(ref p) if p.contains("corrupt-")
        ));
    }

    #[test]
    fn sweep_stale_tmps_removes_only_tmps() {
        let dir = TempDir::new().unwrap();
        let (store, _bus) = store_in(dir.path());
        std::fs::create_dir_all(dir.path().join("shares")).unwrap();
        std::fs::write(dir.path().join("shares").join("a.loro.tmp"), b"x").unwrap();
        std::fs::write(dir.path().join("shares").join("b.loro.tmp"), b"x").unwrap();
        std::fs::write(dir.path().join("shares").join("c.loro"), b"x").unwrap();

        let n = store.sweep_stale_tmps().unwrap();
        assert_eq!(n, 2);
        assert!(dir.path().join("shares").join("c.loro").exists());
    }

    #[test]
    fn sweep_on_missing_dir_is_noop() {
        let dir = TempDir::new().unwrap();
        let (store, _bus) = store_in(dir.path());
        let n = store.sweep_stale_tmps().unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn list_snapshots_returns_ids_only() {
        let dir = TempDir::new().unwrap();
        let (store, _bus) = store_in(dir.path());
        let doc = LoroDoc::new();
        store.save("alpha", &doc).unwrap();
        store.save("beta", &doc).unwrap();
        let mut ids = store.list_snapshots().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }
}
