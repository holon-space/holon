//! LoroDocumentStore - manages the vault's two LoroTree documents.
//!
//! Blocks live in one of two LoroDocs, each with a LoroTree: the GLOBAL doc
//! (notes — the replication set's root container) and the LAYOUT doc (the
//! device-local UI layout). The store handles persistence (each doc is a
//! `.loro` snapshot plus its [`crate::update_log`]) and hands out either doc
//! by [`DocScope`].
//!
//! Legacy per-file methods are retained for backward compat during migration
// ALLOW(compatibility): legacy per-file API shape predates the single-global-doc
// model. Removing requires migrating every per-path caller (org sync, share
// backend, tests); covered separately by the cell-authority cleanup roadmap.
//! but all internally delegate to the global document.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use tokio::sync::RwLock;
use tracing::info;

use crate::CanonicalPath;
use crate::LoroDocument;
use crate::loro_backend::LoroBackend;
use crate::text_undo::TextUndo;

/// Which of the store's two LoroDocuments a caller means.
///
/// The two are disjoint: a block id lives in exactly one of them. `Layout`
/// holds `block:__default__` and its descendants — the device-local UI layout,
/// which is never registered in `ContainerRegistry` and so is structurally
/// outside `replicate_all`'s reach (D68.b).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocScope {
    Global,
    Layout,
}

impl DocScope {
    fn doc_id(self) -> &'static str {
        match self {
            DocScope::Global => GLOBAL_DOC_ID,
            DocScope::Layout => LAYOUT_DOC_ID,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn snapshot_name(self) -> &'static str {
        match self {
            DocScope::Global => GLOBAL_SNAPSHOT_NAME,
            DocScope::Layout => LAYOUT_SNAPSHOT_NAME,
        }
    }
}

/// Manages the vault's two LoroTree documents.
///
/// Every block is stored in one LoroDoc's LoroTree, selected by [`DocScope`].
/// The store handles persistence and provides access to both documents.
///
/// Legacy per-file methods delegate to the global doc for backward compat.
#[derive(Clone)]
pub struct LoroDocumentStore {
    /// The replicated LoroDocument containing the notes LoroTree
    global_doc: Arc<RwLock<Option<Arc<LoroDocument>>>>,
    /// The device-local LoroDocument containing the layout LoroTree
    layout_doc: Arc<RwLock<Option<Arc<LoroDocument>>>>,
    /// Directory where the .loro snapshots are stored
    storage_dir: PathBuf,
    /// Legacy: aliases mapping doc_ids to file paths (kept for org sync compat)
    doc_id_aliases: Arc<RwLock<HashMap<String, CanonicalPath>>>,
    /// Counts snapshot writes to schedule periodic history compaction
    /// (see `persist`). `Arc` so clones share one schedule (the struct is
    /// `Clone`; a per-clone counter would compact on every clone's first save).
    #[cfg(not(target_arch = "wasm32"))]
    save_counter: Arc<std::sync::atomic::AtomicU64>,
    /// What each doc's snapshot and log hold, keyed by doc id; a doc absent
    /// here gets a snapshot on its next save. Held across a whole save, so
    /// two savers cannot write their files in the opposite order to their
    /// exports.
    #[cfg(not(target_arch = "wasm32"))]
    saved: Arc<tokio::sync::Mutex<HashMap<&'static str, Persisted>>>,
    /// Where a dropped torn log tail is disclosed.
    conditions: Option<Arc<holon_conditions::ConditionBus>>,
    /// Peer id to mint both docs under. `None` = the env/random default
    /// in `LoroDocument::new`. Two instances in ONE process must each
    /// inject their own — the env var is process-global and would collide.
    peer_id: Option<u64>,
    /// Stamps the name of a snapshot set aside as undecodable.
    clock: Arc<dyn holon_api::Clock>,
    /// The vault document's text-undo manager, built with the global doc.
    /// Layout has none: a device-local UI arrangement is not the user's typing.
    /// Install-once, and readable WITHOUT an executor: the journal asks for the
    /// text side from inside other runtimes, where an async read would have to
    /// block on one executor from within another.
    text_undo: Arc<std::sync::OnceLock<Arc<TextUndo>>>,
    /// What every paired device holds of the global doc; compaction trims
    /// only below it. Loaded on first use.
    #[cfg(not(target_arch = "wasm32"))]
    peer_versions: Arc<std::sync::OnceLock<Arc<crate::peer_versions::PeerVersions>>>,
}

/// What one doc's snapshot plus update log hold on disk.
#[cfg(not(target_arch = "wasm32"))]
struct Persisted {
    frontiers: loro::Frontiers,
    vv: loro::VersionVector,
    snapshot_bytes: u64,
    log_bytes: u64,
}

/// The replicated document's id and file name — the one document a device
/// pair swaps.
pub const GLOBAL_DOC_ID: &str = "holon_tree";
pub const GLOBAL_SNAPSHOT_NAME: &str = "holon_tree.loro";
const LAYOUT_DOC_ID: &str = "holon_layout";
#[cfg(not(target_arch = "wasm32"))]
const LAYOUT_SNAPSHOT_NAME: &str = "holon_layout.loro";

impl LoroDocumentStore {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self {
            global_doc: Arc::new(RwLock::new(None)),
            layout_doc: Arc::new(RwLock::new(None)),
            storage_dir,
            doc_id_aliases: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(not(target_arch = "wasm32"))]
            save_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(not(target_arch = "wasm32"))]
            saved: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            conditions: None,
            peer_id: None,
            clock: Arc::new(holon_api::SystemClock),
            text_undo: Arc::new(std::sync::OnceLock::new()),
            #[cfg(not(target_arch = "wasm32"))]
            peer_versions: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// The version map of the global doc's paired devices, stored beside its
    /// snapshot.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn peer_versions(&self) -> Result<Arc<crate::peer_versions::PeerVersions>> {
        if let Some(ledger) = self.peer_versions.get() {
            return Ok(ledger.clone());
        }
        let ledger = Arc::new(crate::peer_versions::PeerVersions::open(&self.storage_dir)?);
        Ok(self.peer_versions.get_or_init(|| ledger).clone())
    }

    /// Pin the peer id the global doc is minted under (the session-config
    /// injection seam).
    pub fn with_peer_id(mut self, peer_id: Option<u64>) -> Self {
        self.peer_id = peer_id;
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn holon_api::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Disclose a dropped torn update-log tail on `bus`.
    pub fn with_condition_bus(mut self, bus: Arc<holon_conditions::ConditionBus>) -> Self {
        self.conditions = Some(bus);
        self
    }

    /// The vault document's text-undo manager, once the global doc exists.
    ///
    /// `None` before the first `get_doc(DocScope::Global)`, and on a build that
    /// never opens the vault document. Callers that need it must say so rather
    /// than silently doing nothing.
    pub fn text_undo(&self) -> Option<Arc<TextUndo>> {
        self.text_undo.get().cloned()
    }

    /// Build the vault document's undo manager if it does not exist yet.
    ///
    /// Deliberately NOT done when the document is opened. A Loro manager is a
    /// subscriber, and a subscriber makes Loro materialise an event for every
    /// commit — a cost paid on the whole boot ingest, where there is no typing
    /// to record. Measured on `quick_open_search_at_vault_scale`: 42-54 s with
    /// a manager installed at open against 23-31 s without, on one host.
    ///
    /// So the manager is armed by the first EDITOR cell instead: an editor is
    /// live exactly when typing becomes possible, and ingest never asks for
    /// one. The layout document never gets a manager at all.
    pub async fn ensure_text_undo(&self) -> Result<Arc<TextUndo>> {
        if let Some(existing) = self.text_undo.get() {
            return Ok(existing.clone());
        }
        let doc = self.get_doc(DocScope::Global).await?;
        // Built INSIDE the document's write scope. Loro panics if a subscriber
        // is registered while the document is emitting
        // (`loro-internal/src/utils/subscription.rs` `unwrap_left` on the
        // mid-emit marker), and since increment 0 every commit and import on
        // this document happens inside a write scope — so holding that scope
        // is exactly the mutual exclusion the registration needs. The scope
        // commits nothing; `DocLock` passes reentrant writes through, so a
        // caller that already holds it cannot deadlock here.
        let undo = doc.with_write(crate::write_origin::WriteOrigin::UndoArm, |_txn| {
            Ok(Arc::new(TextUndo::install(doc.clone())))
        })?;
        let _ = self.text_undo.set(undo);
        Ok(self
            .text_undo
            .get()
            .expect("the slot is set above or was already set")
            .clone())
    }

    /// The pinned peer id, if any.
    pub fn peer_id(&self) -> Option<u64> {
        self.peer_id
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn snapshot_path(&self, scope: DocScope) -> PathBuf {
        self.storage_dir.join(scope.snapshot_name())
    }

    fn doc_slot(&self, scope: DocScope) -> &Arc<RwLock<Option<Arc<LoroDocument>>>> {
        match scope {
            DocScope::Global => &self.global_doc,
            DocScope::Layout => &self.layout_doc,
        }
    }

    /// Get one of the two LoroDocuments, loading from disk or creating fresh.
    pub async fn get_doc(&self, scope: DocScope) -> Result<Arc<LoroDocument>> {
        let slot = self.doc_slot(scope);
        // Fast path: already loaded
        {
            let doc = slot.read().await;
            if let Some(d) = doc.as_ref() {
                return Ok(d.clone());
            }
        }

        // Slow path: load or create
        let mut doc_slot = slot.write().await;
        // Double-check after acquiring write lock
        if let Some(d) = doc_slot.as_ref() {
            return Ok(d.clone());
        }
        let doc_id = scope.doc_id();

        #[cfg(not(target_arch = "wasm32"))]
        let doc = {
            let snapshot_path = self.snapshot_path(scope);
            let log_path = crate::update_log::log_path(&snapshot_path);
            if snapshot_path.exists() {
                info!("Loading {doc_id} LoroTree from {}", snapshot_path.display());
                match LoroDocument::load_snapshot_with_peer_id(
                    &snapshot_path,
                    doc_id.to_string(),
                    self.peer_id,
                ) {
                    Ok(loaded) => {
                        let replay = loaded.replay_update_log(&log_path)?;
                        info!(
                            "Replayed {} records of {}",
                            replay.records,
                            log_path.display()
                        );
                        if let Some(torn) = replay.torn_tail {
                            self.disclose_torn_tail(&log_path, torn);
                        }
                        if replay.magic_incomplete {
                            tracing::debug!(
                                "{} held no complete magic; rewriting it",
                                log_path.display()
                            );
                            crate::update_log::reset(&log_path)?;
                        }
                        Arc::new(loaded)
                    }
                    Err(e) => {
                        let error_str = e.to_string();
                        if error_str.contains("Decode error")
                            || error_str.contains("Invalid import data")
                        {
                            self.set_aside(&snapshot_path, &log_path, error_str)?;
                            let fresh = Arc::new(LoroDocument::new_with_peer_id(
                                doc_id.to_string(),
                                self.peer_id,
                            )?);
                            LoroBackend::initialize_schema(&fresh)
                                .await
                                .map_err(|e| anyhow::anyhow!("Failed to init schema: {}", e))?;
                            fresh
                        } else {
                            return Err(e);
                        }
                    }
                }
            } else {
                anyhow::ensure!(
                    !crate::update_log::holds_records(&log_path)?,
                    "{} holds updates but its snapshot {} is missing; delete the Loro store {} \
                     to rebuild it from the org files",
                    log_path.display(),
                    snapshot_path.display(),
                    self.storage_dir.display()
                );
                info!("Creating new {doc_id} LoroTree document");
                let fresh = Arc::new(LoroDocument::new_with_peer_id(
                    doc_id.to_string(),
                    self.peer_id,
                )?);
                LoroBackend::initialize_schema(&fresh)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to init schema: {}", e))?;
                fresh
            }
        };

        #[cfg(target_arch = "wasm32")]
        let doc = {
            info!("Creating in-memory {doc_id} LoroTree (wasm, no snapshot persistence)");
            let fresh = Arc::new(LoroDocument::new_with_peer_id(
                doc_id.to_string(),
                self.peer_id,
            )?);
            LoroBackend::initialize_schema(&fresh)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to init schema: {}", e))?;
            fresh
        };

        *doc_slot = Some(doc.clone());
        Ok(doc)
    }

    // -- Legacy methods that delegate to the global doc --

    /// Register an alias doc_id that maps to a canonical file path.
    /// Kept for org sync. // ALLOW(compatibility): see module-level doc.
    pub async fn register_alias(&self, alias_doc_id: &str, file_path: &Path) {
        let canonical = CanonicalPath::new(file_path);
        self.doc_id_aliases
            .write()
            .await
            .insert(alias_doc_id.to_string(), canonical);
    }

    /// Drop every alias that maps to `file_path`.
    pub async fn forget_aliases_to(&self, file_path: &Path) {
        let canonical = CanonicalPath::new(file_path);
        self.doc_id_aliases
            .write()
            .await
            .retain(|_, path| *path != canonical);
    }

    /// Resolve a doc_id to the global LoroDocument.
    pub async fn resolve_by_doc_id(&self, _: &str) -> Option<Arc<LoroDocument>> {
        self.get_doc(DocScope::Global).await.ok() // ALLOW(ok): doc may not be initialized
    }

    /// Resolve an alias doc_id to its canonical file path.
    pub async fn resolve_alias_to_path(&self, doc_id: &str) -> Option<PathBuf> {
        let aliases = self.doc_id_aliases.read().await;
        aliases.get(doc_id).map(|cp| cp.to_path_buf())
    }

    /// Legacy: get or load a document for a file path.
    /// Now always returns the global doc.
    pub async fn get_or_load(&mut self, _: &Path) -> Result<Arc<LoroDocument>> {
        self.get_doc(DocScope::Global).await
    }

    /// Persist every loaded document whose committed state is not on disk yet;
    /// a document already saved at its current frontiers is skipped. Once this
    /// returns, what it persisted survives a crash and a power loss.
    ///
    /// A save appends the changes since the last one to the document's update
    /// log, and writes a fresh snapshot (resetting the log) once the log would
    /// outgrow the snapshot, so a save costs the size of its change plus,
    /// amortised, at most as much again.
    ///
    /// Anything that makes a Loro change visible outside the document (the SQL
    /// projection, a session quit) calls this first, so the state on disk is
    /// never behind what the rest of the system already reflects.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn save_all(&self) -> Result<()> {
        let mut saved = self.saved.lock().await;
        for (scope, doc) in self.loaded_docs().await {
            let frontiers = doc.with_read(|d| Ok(d.oplog_frontiers()))?;
            if saved.get(scope.doc_id()).map(|p| &p.frontiers) != Some(&frontiers) {
                self.persist(scope, &doc, &mut saved, false)?;
            }
        }
        Ok(())
    }

    /// The document `scope`'s snapshot and update log persist. Held under the
    /// save lock so a save cannot swap the snapshot and reset the log between
    /// the two reads.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn read_persisted(
        &self,
        scope: DocScope,
    ) -> Result<(loro::LoroDoc, crate::update_log::Replay)> {
        let _saved = self.saved.lock().await;
        crate::update_log::read_persisted(&self.snapshot_path(scope))
    }

    /// Write a snapshot of every loaded document and empty its update log, so
    /// the snapshot alone holds the document. For a quit, and before anything
    /// that reads or moves snapshot files on its own.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn checkpoint(&self) -> Result<()> {
        let mut saved = self.saved.lock().await;
        self.checkpoint_locked(&mut saved).await
    }

    /// Run `swap` on the storage dir while no save can run, with the global
    /// document checkpointed first: its snapshot is the whole document and its
    /// log holds no record. The next save writes a fresh snapshot, so whatever
    /// global snapshot `swap` leaves in place is replaced, never appended to.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn replace_global_files<R>(
        &self,
        swap: impl FnOnce(&Path) -> Result<R>,
    ) -> Result<R> {
        let mut saved = self.saved.lock().await;
        self.checkpoint_locked(&mut saved).await?;
        let result = swap(&self.storage_dir);
        saved.remove(DocScope::Global.doc_id());
        result
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn checkpoint_locked(&self, saved: &mut HashMap<&'static str, Persisted>) -> Result<()> {
        for (scope, doc) in self.loaded_docs().await {
            let frontiers = doc.with_read(|d| Ok(d.oplog_frontiers()))?;
            let current = saved
                .get(scope.doc_id())
                .is_some_and(|p| p.frontiers == frontiers && p.log_bytes == 0);
            if !current {
                self.persist(scope, &doc, saved, true)?;
            }
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn loaded_docs(&self) -> Vec<(DocScope, Arc<LoroDocument>)> {
        let mut docs = Vec::new();
        for scope in [DocScope::Global, DocScope::Layout] {
            if let Some(doc) = self.doc_slot(scope).read().await.as_ref() {
                docs.push((scope, doc.clone()));
            }
        }
        docs
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn persist(
        &self,
        scope: DocScope,
        doc: &LoroDocument,
        saved: &mut HashMap<&'static str, Persisted>,
        snapshot: bool,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;

        use anyhow::Context;

        use crate::loro_document::SaveExport;
        use crate::update_log;

        let path = self.snapshot_path(scope);
        let log = update_log::log_path(&path);
        if let Some(on_disk) = saved.get_mut(scope.doc_id())
            && !snapshot
        {
            let delta = doc.export_for_save(SaveExport::UpdatesSince(&on_disk.vv))?;
            let record = update_log::RECORD_HEADER + delta.bytes.len() as u64;
            if on_disk.log_bytes + record <= on_disk.snapshot_bytes {
                let acked_len = update_log::MAGIC.len() as u64 + on_disk.log_bytes;
                if let Some(stump) = update_log::append(&log, &delta.bytes, acked_len)? {
                    self.disclose_dropped(
                        &log,
                        stump.bytes,
                        "bytes behind the last acknowledged record".to_string(),
                    );
                }
                on_disk.frontiers = delta.frontiers;
                on_disk.vv = delta.vv;
                on_disk.log_bytes += record;
                return Ok(());
            }
        }

        // Periodic history compaction: every Nth snapshot (incl. the first of
        // a session, which sheds history accumulated in prior sessions) of the
        // replicated doc trims the history every paired device already holds;
        // the device-local layout doc trims all of it. Holon undo replays the
        // inverse-command log, so trimmed Loro history is never needed
        // locally. Kill-switch: HOLON_LORO_COMPACT=off.
        const COMPACT_EVERY: u64 = 64;
        let n = self.save_counter.fetch_add(1, Ordering::Relaxed);
        let compact = std::env::var("HOLON_LORO_COMPACT")
            .map(|v| v != "off")
            .unwrap_or(true)
            && n.is_multiple_of(COMPACT_EVERY);
        let floor = match (compact, scope) {
            (true, DocScope::Global) => self.peer_versions()?.floor(),
            _ => None,
        };
        let full = doc.export_for_save(match (compact, scope, &floor) {
            (true, DocScope::Layout, _) => SaveExport::CompactSnapshot,
            (true, DocScope::Global, Some(floor)) => SaveExport::CompactSnapshotBelow(floor),
            _ => SaveExport::Snapshot,
        })?;
        std::fs::create_dir_all(&self.storage_dir)
            .with_context(|| format!("create the Loro dir {}", self.storage_dir.display()))?;
        holon_filesystem::fs_port::write_durable_blocking(&path, &full.bytes)
            .with_context(|| format!("save {} to {}", scope.doc_id(), path.display()))?;
        update_log::reset(&log)?;
        saved.insert(
            scope.doc_id(),
            Persisted {
                frontiers: full.frontiers,
                vv: full.vv,
                snapshot_bytes: full.bytes.len() as u64,
                log_bytes: 0,
            },
        );
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disclose_torn_tail(&self, log: &Path, torn: crate::update_log::TornTail) {
        self.disclose_dropped(
            log,
            torn.bytes,
            format!("{} at byte {}", torn.reason, torn.offset),
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disclose_dropped(&self, log: &Path, bytes: u64, reason: String) {
        tracing::warn!("dropped {bytes} bytes of {}: {reason}", log.display());
        if let Some(bus) = &self.conditions {
            bus.emit(holon_conditions::Condition {
                subject: log.display().to_string(),
                reason: holon_conditions::ConditionKind::LoroUpdateLogTailDropped { bytes, reason },
            });
        }
    }

    /// Renames an undecodable snapshot and its log next to themselves, named
    /// with the time and the reason, so a fresh store can start in their place.
    #[cfg(not(target_arch = "wasm32"))]
    fn set_aside(&self, snapshot: &Path, log: &Path, cause: String) -> Result<()> {
        let stamp = chrono::DateTime::from_timestamp_millis(self.clock.now_millis())
            .expect("the clock reads a representable time")
            .format("%Y%m%dT%H%M%S%.3fZ");
        let mut moved = Vec::new();
        for file in [snapshot, log] {
            if !file.exists() {
                continue;
            }
            let mut target = file.as_os_str().to_owned();
            target.push(format!(".undecodable-{stamp}"));
            let target = PathBuf::from(target);
            anyhow::ensure!(
                !target.exists(),
                "cannot set the undecodable {} aside: {} already exists",
                file.display(),
                target.display()
            );
            std::fs::rename(file, &target).with_context(|| {
                format!(
                    "set the undecodable {} aside as {}",
                    file.display(),
                    target.display()
                )
            })?;
            moved.push(target.display().to_string());
        }
        tracing::warn!(
            "{} is undecodable ({cause}); moved to {moved:?}, starting a fresh store",
            snapshot.display()
        );
        if let Some(bus) = &self.conditions {
            bus.emit(holon_conditions::Condition {
                subject: snapshot.display().to_string(),
                reason: holon_conditions::ConditionKind::LoroSnapshotSetAside { moved, cause },
            });
        }
        Ok(())
    }

    /// No-op on wasm: the snapshot writer is the native atomic-replacement
    /// helper (`holon_filesystem::fs_port`), so both documents stay in memory
    /// for the lifetime of the instance.
    #[cfg(target_arch = "wasm32")]
    pub async fn save_all(&self) -> Result<()> {
        Ok(())
    }

    /// No-op on wasm, like [`Self::save_all`].
    #[cfg(target_arch = "wasm32")]
    pub async fn checkpoint(&self) -> Result<()> {
        Ok(())
    }

    pub async fn save(&self, _: &Path) -> Result<()> {
        self.save_all().await
    }

    pub async fn remove(&mut self, _: &Path) {
        // No-op: we don't remove the global doc
    }

    pub async fn get(&self, _: &Path) -> Option<Arc<LoroDocument>> {
        self.get_doc(DocScope::Global).await.ok() // ALLOW(ok): doc may not be initialized
    }

    pub async fn get_loaded_paths(&self) -> Vec<CanonicalPath> {
        // Legacy: return storage_dir as the single "loaded path"
        vec![CanonicalPath::new(&self.storage_dir)]
    }

    pub async fn iter(&self) -> Vec<(CanonicalPath, Arc<LoroDocument>)> {
        if let Ok(doc) = self.get_doc(DocScope::Global).await {
            vec![(CanonicalPath::new(&self.storage_dir), doc)]
        } else {
            vec![]
        }
    }

    pub async fn get_all_aliases(&self) -> Vec<(String, PathBuf)> {
        let aliases = self.doc_id_aliases.read().await;
        aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.to_path_buf()))
            .collect()
    }

    /// Legacy: load existing .loro files. Now just loads the global snapshot.
    pub async fn load_all_existing(
        &mut self,
        _: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
        // Just ensure both docs are loaded
        self.get_doc(DocScope::Global).await?;
        self.get_doc(DocScope::Layout).await?;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_global_doc_creates_new() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = LoroDocumentStore::new(temp_dir.path().to_path_buf());
        let doc = store.get_doc(DocScope::Global).await?;
        assert_eq!(doc.doc_id(), GLOBAL_DOC_ID);
        Ok(())
    }

    #[tokio::test]
    async fn test_global_doc_reuses() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = LoroDocumentStore::new(temp_dir.path().to_path_buf());
        let doc1 = store.get_doc(DocScope::Global).await?;
        let doc2 = store.get_doc(DocScope::Global).await?;
        assert!(Arc::ptr_eq(&doc1, &doc2));
        Ok(())
    }

    #[tokio::test]
    async fn test_save_and_load() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store_dir = temp_dir.path().to_path_buf();
        let store = LoroDocumentStore::new(store_dir.clone());
        let doc1 = store.get_doc(DocScope::Global).await?;

        doc1.insert_text("test", 0, "Hello")?;
        store.save_all().await?;

        // New store should load persisted data
        let store2 = LoroDocumentStore::new(store_dir);
        let doc2 = store2.get_doc(DocScope::Global).await?;
        let text = doc2.get_text("test")?;
        assert_eq!(text, "Hello");
        Ok(())
    }

    #[tokio::test]
    async fn test_legacy_get_or_load_returns_global() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut store = LoroDocumentStore::new(temp_dir.path().to_path_buf());
        let doc = store.get_or_load(Path::new("whatever.org")).await?;
        assert_eq!(doc.doc_id(), GLOBAL_DOC_ID);
        Ok(())
    }

    #[derive(Clone, Debug)]
    enum Damage {
        Replace(Vec<u8>),
        Truncate(prop::sample::Index),
        Flip(prop::sample::Index, u8),
    }

    fn damage() -> impl Strategy<Value = Damage> {
        prop_oneof![
            prop::collection::vec(any::<u8>(), 0..64).prop_map(Damage::Replace),
            any::<prop::sample::Index>().prop_map(Damage::Truncate),
            (any::<prop::sample::Index>(), 1..=u8::MAX).prop_map(|(i, m)| Damage::Flip(i, m)),
        ]
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .flat_map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files_under(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    }

    fn holder_of(dir: &Path, bytes: &[u8]) -> Option<PathBuf> {
        files_under(dir)
            .into_iter()
            .find(|f| std::fs::read(f).unwrap() == bytes)
    }

    /// Whatever a damaged snapshot and its log hold when the store opens
    /// stays on disk byte-equal, Holon still boots, and when the files left
    /// their place a notice names where they are now, under a name stamped
    /// with the injected clock's time.
    async fn opening_a_damaged_snapshot_keeps_its_bytes(
        damage: &Damage,
        with_log: bool,
        at_millis: i64,
    ) {
        let dir = TempDir::new().unwrap();
        let store = LoroDocumentStore::new(dir.path().to_path_buf());
        let doc = store.get_doc(DocScope::Global).await.unwrap();
        doc.insert_text("body", 0, &"seed ".repeat(2_000)).unwrap();
        store.save_all().await.unwrap();
        if with_log {
            doc.insert_text("body", 0, "logged ").unwrap();
            store.save_all().await.unwrap();
        }
        drop((store, doc));

        let snapshot = dir.path().join(GLOBAL_SNAPSHOT_NAME);
        let log = crate::update_log::log_path(&snapshot);
        let mut bytes = std::fs::read(&snapshot).unwrap();
        match damage {
            Damage::Replace(garbage) => bytes = garbage.clone(),
            Damage::Truncate(at) => bytes.truncate(at.index(bytes.len())),
            Damage::Flip(at, mask) => {
                let i = at.index(bytes.len());
                bytes[i] ^= mask;
            }
        }
        std::fs::write(&snapshot, &bytes).unwrap();
        let mut kept = vec![("snapshot", snapshot.clone(), bytes)];
        if log.exists() {
            kept.push(("log", log.clone(), std::fs::read(&log).unwrap()));
        }

        let bus = Arc::new(holon_conditions::ConditionBus::new());
        LoroDocumentStore::new(dir.path().to_path_buf())
            .with_clock(Arc::new(holon_api::TestClock::new(at_millis)))
            .with_condition_bus(bus.clone())
            .get_doc(DocScope::Global)
            .await
            .unwrap_or_else(|e| panic!("Holon must boot over {damage:?}: {e:#}"));

        let notices = format!("{:?}", bus.current());
        for (what, original, kept) in kept {
            let holder = holder_of(dir.path(), &kept)
                .unwrap_or_else(|| panic!("the {what} bytes are gone after {damage:?}"));
            assert!(
                holder == original || notices.contains(&holder.display().to_string()),
                "the {what} moved to {} after {damage:?}, but no notice names it: {notices}",
                holder.display()
            );
            if holder != original {
                let stamp = chrono::DateTime::from_timestamp_millis(at_millis)
                    .unwrap()
                    .format("%Y%m%dT%H%M%S%.3fZ");
                assert_eq!(
                    holder.file_name().unwrap().to_string_lossy(),
                    format!(
                        "{}.undecodable-{stamp}",
                        original.file_name().unwrap().to_string_lossy()
                    ),
                    "the {what} set aside at {at_millis} ms must carry the clock's time"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_snapshot_of_garbage_keeps_its_bytes() {
        for with_log in [false, true] {
            opening_a_damaged_snapshot_keeps_its_bytes(
                &Damage::Replace(b"not loro".to_vec()),
                with_log,
                1_762_560_000_123,
            )
            .await;
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 40, failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn a_damaged_snapshot_keeps_its_bytes(
            damage in damage(),
            with_log in any::<bool>(),
            at_millis in 0..4_102_444_800_000i64,
        ) {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(opening_a_damaged_snapshot_keeps_its_bytes(&damage, with_log, at_millis));
        }
    }
}
