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
    save_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Peer id to mint both docs under. `None` = the env/random default
    /// in `LoroDocument::new`. Two instances in ONE process must each
    /// inject their own — the env var is process-global and would collide.
    peer_id: Option<u64>,
}

/// The replicated document's id and file name — the one document a device
/// pair swaps.
pub const GLOBAL_DOC_ID: &str = "holon_tree";
pub const GLOBAL_SNAPSHOT_NAME: &str = "holon_tree.loro";
const LAYOUT_DOC_ID: &str = "holon_layout";
const LAYOUT_SNAPSHOT_NAME: &str = "holon_layout.loro";

impl LoroDocumentStore {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self {
            global_doc: Arc::new(RwLock::new(None)),
            layout_doc: Arc::new(RwLock::new(None)),
            storage_dir,
            doc_id_aliases: Arc::new(RwLock::new(HashMap::new())),
            save_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            peer_id: None,
        }
    }

    /// Pin the peer id the global doc is minted under (the session-config
    /// injection seam).
    pub fn with_peer_id(mut self, peer_id: Option<u64>) -> Self {
        self.peer_id = peer_id;
        self
    }

    /// The pinned peer id, if any.
    pub fn peer_id(&self) -> Option<u64> {
        self.peer_id
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

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

        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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

        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let doc = {
            info!("Creating in-memory {doc_id} LoroTree (wasm32 demo, no persistence)");
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

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub async fn save_all(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
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

        for scope in [DocScope::Global, DocScope::Layout] {
            let doc = self.doc_slot(scope).read().await;
            let Some(d) = doc.as_ref() else { continue };
            let path = self.snapshot_path(scope);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if compact {
                d.save_compact_to_file(&path)?;
            } else {
                d.save_to_file(&path)?;
            }
        }
        Ok(())
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    pub async fn save_all(&self) -> Result<()> {
        // wasm32 demo is in-memory only; no persistence.
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
