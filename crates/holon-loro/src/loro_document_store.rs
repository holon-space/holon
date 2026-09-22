//! LoroDocumentStore - manages the vault's two LoroTree documents.
//!
//! Blocks live in one of two LoroDocs, each with a LoroTree: the GLOBAL doc
//! (notes — the replication set's root container) and the LAYOUT doc (the
//! device-local UI layout). The store handles persistence (saving/loading each
//! `.loro` snapshot) and hands out either doc by [`DocScope`].
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
    /// Counts `save_all` calls to schedule periodic history compaction
    /// (see `save_all`). `Arc` so clones share one schedule (the struct is
    /// `Clone`; a per-clone counter would compact on every clone's first save).
    #[cfg(not(target_arch = "wasm32"))]
    save_counter: Arc<std::sync::atomic::AtomicU64>,
    /// The oplog frontiers each snapshot file holds, keyed by doc id. Held
    /// across a whole `save_all`, so two savers cannot rename their files in
    /// the opposite order to their exports.
    #[cfg(not(target_arch = "wasm32"))]
    saved: Arc<tokio::sync::Mutex<HashMap<&'static str, loro::Frontiers>>>,
    /// Peer id to mint both docs under. `None` = the env/random default
    /// in `LoroDocument::new`. Two instances in ONE process must each
    /// inject their own — the env var is process-global and would collide.
    peer_id: Option<u64>,
    /// The vault document's text-undo manager, built with the global doc.
    /// Layout has none: a device-local UI arrangement is not the user's typing.
    /// Install-once, and readable WITHOUT an executor: the journal asks for the
    /// text side from inside other runtimes, where an async read would have to
    /// block on one executor from within another.
    text_undo: Arc<std::sync::OnceLock<Arc<TextUndo>>>,
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
            peer_id: None,
            text_undo: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Pin the peer id the global doc is minted under (the session-config
    /// injection seam).
    pub fn with_peer_id(mut self, peer_id: Option<u64>) -> Self {
        self.peer_id = peer_id;
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
            if snapshot_path.exists() {
                info!("Loading {doc_id} LoroTree from {}", snapshot_path.display());
                match LoroDocument::load_from_file_with_peer_id(
                    &snapshot_path,
                    doc_id.to_string(),
                    self.peer_id,
                ) {
                    Ok(loaded) => Arc::new(loaded),
                    Err(e) => {
                        let error_str = e.to_string();
                        if error_str.contains("Decode error")
                            || error_str.contains("Invalid import data")
                        {
                            tracing::warn!(
                                "Corrupted snapshot at {}: {}. Recreating.",
                                snapshot_path.display(),
                                e
                            );
                            let _ = std::fs::remove_file(&snapshot_path);
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

    /// Write every loaded document whose committed state is not on disk yet;
    /// a document already saved at its current frontiers is skipped.
    ///
    /// Anything that makes a Loro change visible outside the document (the SQL
    /// projection, a session quit) calls this first, so the snapshot on disk is
    /// never behind what the rest of the system already reflects.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn save_all(&self) -> Result<()> {
        use std::sync::atomic::Ordering;

        use anyhow::Context;

        let mut saved = self.saved.lock().await;
        let mut behind = Vec::new();
        for scope in [DocScope::Global, DocScope::Layout] {
            let slot = self.doc_slot(scope).read().await;
            let Some(doc) = slot.as_ref() else { continue };
            // Read before the export, so the recorded frontiers never claim
            // more than the file holds.
            let frontiers = doc.with_read(|d| Ok(d.oplog_frontiers()))?;
            if saved.get(scope.doc_id()) != Some(&frontiers) {
                behind.push((scope, doc.clone(), frontiers));
            }
        }
        if behind.is_empty() {
            return Ok(());
        }

        // Periodic history compaction: every Nth save (incl. the first save of
        // a session, which sheds history accumulated in prior sessions) write a
        // shallow snapshot instead of a full one. Holon undo replays the
        // inverse-command log, so trimmed Loro history is never needed locally;
        // stale P2P peers get a full snapshot via the delta-export guard in
        // `iroh_sync_adapter`. Kill-switch: HOLON_LORO_COMPACT=off.
        const COMPACT_EVERY: u64 = 64;
        let n = self.save_counter.fetch_add(1, Ordering::Relaxed);
        let compact = std::env::var("HOLON_LORO_COMPACT")
            .map(|v| v != "off")
            .unwrap_or(true)
            && n.is_multiple_of(COMPACT_EVERY);

        for (scope, doc, frontiers) in behind {
            let path = self.snapshot_path(scope);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create the Loro dir {}", parent.display()))?;
            }
            if compact {
                doc.save_compact_to_file(&path)
            } else {
                doc.save_to_file(&path)
            }
            .with_context(|| format!("save {} to {}", scope.doc_id(), path.display()))?;
            saved.insert(scope.doc_id(), frontiers);
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
}
