//! LoroDocumentStore - manages the single global LoroTree document.
//!
//! All blocks live in one LoroDoc with a LoroTree. The store handles persistence
//! (saving/loading the `.loro` snapshot) and provides access to the global doc.
//!
//! Legacy per-file methods are retained for backward compatibility during migration
//! but all internally delegate to the single global document.

use crate::api::LoroBackend;
use crate::sync::LoroDocument;
use crate::sync::canonical_path::CanonicalPath;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Manages the single global LoroTree document.
///
/// All blocks are stored in one LoroDoc's LoroTree. The store handles
/// persistence and provides access to the global document.
///
/// Legacy per-file methods delegate to the global doc for backward compat.
#[derive(Clone)]
pub struct LoroDocumentStore {
    /// The single global LoroDocument containing the LoroTree
    global_doc: Arc<RwLock<Option<Arc<LoroDocument>>>>,
    /// Directory where the .loro snapshot is stored
    storage_dir: PathBuf,
    /// Legacy: aliases mapping doc_ids to file paths (kept for org sync compat)
    doc_id_aliases: Arc<RwLock<HashMap<String, CanonicalPath>>>,
}

const GLOBAL_DOC_ID: &str = "holon_tree";
const GLOBAL_SNAPSHOT_NAME: &str = "holon_tree.loro";

impl LoroDocumentStore {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self {
            global_doc: Arc::new(RwLock::new(None)),
            storage_dir,
            doc_id_aliases: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    fn snapshot_path(&self) -> PathBuf {
        self.storage_dir.join(GLOBAL_SNAPSHOT_NAME)
    }

    /// Get the global LoroDocument, loading from disk or creating fresh.
    pub async fn get_global_doc(&self) -> Result<Arc<LoroDocument>> {
        // Fast path: already loaded
        {
            let doc = self.global_doc.read().await;
            if let Some(d) = doc.as_ref() {
                return Ok(d.clone());
            }
        }

        // Slow path: load or create
        let mut doc_slot = self.global_doc.write().await;
        // Double-check after acquiring write lock
        if let Some(d) = doc_slot.as_ref() {
            return Ok(d.clone());
        }

        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let doc = {
            let snapshot_path = self.snapshot_path();
            if snapshot_path.exists() {
                info!("Loading global LoroTree from {}", snapshot_path.display());
                match LoroDocument::load_from_file(&snapshot_path, GLOBAL_DOC_ID.to_string()) {
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
                            let fresh = Arc::new(LoroDocument::new(GLOBAL_DOC_ID.to_string())?);
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
                info!("Creating new global LoroTree document");
                let fresh = Arc::new(LoroDocument::new(GLOBAL_DOC_ID.to_string())?);
                LoroBackend::initialize_schema(&fresh)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to init schema: {}", e))?;
                fresh
            }
        };

        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let doc = {
            info!("Creating in-memory global LoroTree (wasm32 demo, no persistence)");
            let fresh = Arc::new(LoroDocument::new(GLOBAL_DOC_ID.to_string())?);
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
    /// Kept for org sync backward compatibility.
    pub async fn register_alias(&self, alias_doc_id: &str, file_path: &Path) {
        let canonical = CanonicalPath::new(file_path);
        self.doc_id_aliases
            .write()
            .await
            .insert(alias_doc_id.to_string(), canonical);
    }

    /// Resolve a doc_id to the global LoroDocument.
    pub async fn resolve_by_doc_id(&self, _doc_id: &str) -> Option<Arc<LoroDocument>> {
        self.get_global_doc().await.ok() // ALLOW(ok): doc may not be initialized
    }

    /// Resolve an alias doc_id to its canonical file path.
    pub async fn resolve_alias_to_path(&self, doc_id: &str) -> Option<PathBuf> {
        let aliases = self.doc_id_aliases.read().await;
        aliases.get(doc_id).map(|cp| cp.to_path_buf())
    }

    /// Legacy: get or load a document for a file path.
    /// Now always returns the global doc.
    pub async fn get_or_load(&mut self, _path: &Path) -> Result<Arc<LoroDocument>> {
        self.get_global_doc().await
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    pub async fn save_all(&self) -> Result<()> {
        let doc = self.global_doc.read().await;
        if let Some(d) = doc.as_ref() {
            let path = self.snapshot_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            d.save_to_file(&path)?;
        }
        Ok(())
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    pub async fn save_all(&self) -> Result<()> {
        // wasm32 demo is in-memory only; no persistence.
        Ok(())
    }

    pub async fn save(&self, _path: &Path) -> Result<()> {
        self.save_all().await
    }

    pub async fn remove(&mut self, _path: &Path) {
        // No-op: we don't remove the global doc
    }

    pub async fn get(&self, _path: &Path) -> Option<Arc<LoroDocument>> {
        self.get_global_doc().await.ok() // ALLOW(ok): doc may not be initialized
    }

    pub async fn get_loaded_paths(&self) -> Vec<CanonicalPath> {
        // Legacy: return storage_dir as the single "loaded path"
        vec![CanonicalPath::new(&self.storage_dir)]
    }

    pub async fn iter(&self) -> Vec<(CanonicalPath, Arc<LoroDocument>)> {
        if let Ok(doc) = self.get_global_doc().await {
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
        _root_dir: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
        // Just ensure the global doc is loaded
        self.get_global_doc().await?;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_global_doc_creates_new() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = LoroDocumentStore::new(temp_dir.path().to_path_buf());
        let doc = store.get_global_doc().await?;
        assert_eq!(doc.doc_id(), GLOBAL_DOC_ID);
        Ok(())
    }

    #[tokio::test]
    async fn test_global_doc_reuses() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = LoroDocumentStore::new(temp_dir.path().to_path_buf());
        let doc1 = store.get_global_doc().await?;
        let doc2 = store.get_global_doc().await?;
        assert!(Arc::ptr_eq(&doc1, &doc2));
        Ok(())
    }

    #[tokio::test]
    async fn test_save_and_load() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store_dir = temp_dir.path().to_path_buf();
        let store = LoroDocumentStore::new(store_dir.clone());
        let doc1 = store.get_global_doc().await?;

        doc1.insert_text("test", 0, "Hello")?;
        store.save_all().await?;

        // New store should load persisted data
        let store2 = LoroDocumentStore::new(store_dir);
        let doc2 = store2.get_global_doc().await?;
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
