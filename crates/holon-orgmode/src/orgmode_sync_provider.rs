//! Stream-based OrgModeSyncProvider
//!
//! This sync provider scans an org-mode directory and emits changes on typed
//! streams. Architecture:
//! - ONE sync() call → multiple typed streams (directories, files, blocks)
//! - Uses file content hashes for change detection
//! - Fire-and-forget operations - updates arrive via streams
//!
//! Every sync is a FULL walk of the local tree (listing is cheap; there is no
//! remote resume cursor). The provider keeps its own durable base -- the
//! `SyncState` JSON persisted in the `SyncTokenStore` -- solely to compute the
//! delta against the previous walk: which entries are `Created` vs `Updated`,
//! and which vanished and must be emitted as `Deleted`. The `position`
//! argument of `sync()` is therefore ignored; the base is always loaded from
//! the token store and saved back by the provider itself after emitting.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use holon_api::BatchMetadata;
use holon_api::Change;
use holon_api::ChangeOrigin;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_api::WithMetadata;
use holon_core::generate_sync_operation;
use holon_core::storage::types::StorageEntity;
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_filesystem::directory::ChangesWithMetadata;
use holon_filesystem::directory::Directory;
use holon_filesystem::directory::DirectoryChangeProvider;
use holon_filesystem::directory::ROOT_ID;
use holon_filesystem::File;
use holon_filesystem::FileSystem;
use tokio::sync::broadcast;

use crate::parser::compute_content_hash;
use crate::parser::generate_directory_id;
use crate::parser::generate_file_id;

/// Sync state stored as JSON in token store
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SyncState {
    /// Map of file paths to their content hashes
    file_hashes: HashMap<String, String>,
    /// Map of directory paths
    known_dirs: HashMap<String, bool>,
}

/// Stream-based OrgModeSyncProvider that scans directories and emits changes on
/// typed streams
pub struct OrgModeSyncProvider {
    root_directory: PathBuf,
    token_store: Arc<dyn SyncTokenStore>,
    directory_tx: broadcast::Sender<ChangesWithMetadata<Directory>>,
    file_tx: broadcast::Sender<ChangesWithMetadata<File>>,
    fs: Arc<dyn FileSystem>,
    /// Serializes the load-base -> scan -> save-base read-modify-write of
    /// `sync` / `sync_changes` so concurrent calls cannot lose deletions.
    sync_lock: tokio::sync::Mutex<()>,
}

impl OrgModeSyncProvider {
    pub fn new(
        root_directory: PathBuf,
        token_store: Arc<dyn SyncTokenStore>,
        fs: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            root_directory,
            token_store,
            directory_tx: broadcast::channel(1000).0,
            file_tx: broadcast::channel(1000).0,
            fs,
            sync_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn subscribe_directories(&self) -> broadcast::Receiver<ChangesWithMetadata<Directory>> {
        self.directory_tx.subscribe()
    }

    pub fn subscribe_files(&self) -> broadcast::Receiver<ChangesWithMetadata<File>> {
        self.file_tx.subscribe()
    }

    /// Load sync state from token store
    async fn load_state(&self) -> Result<SyncState> {
        let position = self
            .token_store
            .load_token(self.provider_name())
            .await?
            .unwrap_or(StreamPosition::Beginning);

        match position {
            StreamPosition::Beginning => Ok(SyncState::default()),
            StreamPosition::Version(bytes) => {
                let state: SyncState = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Failed to parse sync state: {}", e))?;
                Ok(state)
            }
        }
    }

    /// Perform directory scan and compute changes
    async fn scan_and_compute_changes(
        &self,
        old_state: &SyncState,
    ) -> Result<(SyncState, Vec<Change<Directory>>, Vec<Change<File>>)> {
        let origin = ChangeOrigin::remote_with_current_span();
        let mut new_state = SyncState::default();
        let mut dir_changes = Vec::new();
        let mut file_changes = Vec::new();

        // Track what we've seen to detect deletions
        let mut seen_dirs: HashMap<String, bool> = HashMap::new();
        let mut seen_files: HashMap<String, bool> = HashMap::new();

        let scanned = crate::file_watcher::scan_directory(self.fs.as_ref(), &self.root_directory)
            .await
            .map_err(|e| format!("Failed to scan {}: {e}", self.root_directory.display()))?;

        for path in &scanned.directories {
            let dir_id = generate_directory_id(path, &self.root_directory);
            seen_dirs.insert(dir_id.clone(), true);

            let parent_id = path
                .parent()
                .map(|p| {
                    if p == self.root_directory {
                        ROOT_ID.to_string()
                    } else {
                        generate_directory_id(p, &self.root_directory)
                    }
                })
                .unwrap_or_else(|| ROOT_ID.to_string());

            let depth = path
                .strip_prefix(&self.root_directory)
                .map(|p| p.components().count() as i64)
                .unwrap_or(1);

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            if !old_state.known_dirs.contains_key(&dir_id) {
                let dir = Directory::new(dir_id.clone(), name, parent_id, depth);
                dir_changes.push(Change::Created {
                    data: dir,
                    origin: origin.clone(),
                });
            }

            new_state.known_dirs.insert(dir_id, true);
        }

        let canonical_root = self
            .fs
            .canonicalize(&self.root_directory)
            .unwrap_or_else(|_| self.root_directory.clone());
        for path in &scanned.files {
            let canonical_path = self.fs.canonicalize(path).unwrap_or_else(|_| path.clone());
            let file_id = generate_file_id(&canonical_path, &canonical_root).to_string();
            seen_files.insert(file_id.clone(), true);

            let content = self
                .fs
                .read_to_string(path)
                .await
                .with_context(|| format!("Failed to read {}", path.display()))?;

            let content_hash = compute_content_hash(&content);
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let file_changed = old_state
                .file_hashes
                .get(&file_id)
                .map(|old_hash| old_hash != &content_hash)
                .unwrap_or(true);

            if file_changed {
                let parent_id = path
                    .parent()
                    .map(|p| {
                        if p == self.root_directory {
                            ROOT_ID.to_string()
                        } else {
                            generate_directory_id(p, &self.root_directory)
                        }
                    })
                    .unwrap_or_else(|| ROOT_ID.to_string());

                let file = File::new(
                    file_id.clone(),
                    file_name.clone(),
                    parent_id.clone(),
                    content_hash.clone(),
                    None,
                );
                let is_new = !old_state.file_hashes.contains_key(&file_id);
                if is_new {
                    file_changes.push(Change::Created {
                        data: file,
                        origin: origin.clone(),
                    });
                } else {
                    file_changes.push(Change::Updated {
                        id: file_id.clone(),
                        data: file,
                        origin: origin.clone(),
                    });
                }
            }

            new_state.file_hashes.insert(file_id, content_hash);
        }

        tracing::info!(
            "[OrgModeSyncProvider] Scan complete: {} directories, {} files found",
            scanned.directories.len(),
            scanned.files.len()
        );

        // Detect deleted directories
        for old_dir_id in old_state.known_dirs.keys() {
            if !seen_dirs.contains_key(old_dir_id) {
                dir_changes.push(Change::Deleted {
                    id: old_dir_id.clone(),
                    origin: origin.clone(),
                });
            }
        }

        // Detect deleted files (and their blocks)
        for old_file_id in old_state.file_hashes.keys() {
            if !seen_files.contains_key(old_file_id) {
                file_changes.push(Change::Deleted {
                    id: old_file_id.clone(),
                    origin: origin.clone(),
                });
                // Note: Blocks from deleted files should be cleaned up
                // In production, we'd track block IDs per file
            }
        }

        Ok((new_state, dir_changes, file_changes))
    }

    /// Extract file paths from FieldDeltas
    ///
    /// FieldDeltas now include a "file_path" field for operations that modify
    /// files. This extracts unique file paths from those FieldDeltas.
    fn extract_file_paths_from_deltas(
        &self,
        changes: &[FieldDelta],
    ) -> std::collections::HashSet<PathBuf> {
        let mut file_paths = std::collections::HashSet::new();

        for delta in changes {
            // Look for FieldDeltas with field name "file_path"
            if delta.field == "file_path" {
                // Extract file path from new_value (or old_value if new_value is null)
                if let Value::String(path_str) = &delta.new_value {
                    if !path_str.is_empty() {
                        file_paths.insert(PathBuf::from(path_str));
                    }
                } else if let Value::String(path_str) = &delta.old_value {
                    if !path_str.is_empty() {
                        file_paths.insert(PathBuf::from(path_str));
                    }
                }
            }
        }

        file_paths
    }
}

impl DirectoryChangeProvider for OrgModeSyncProvider {
    fn subscribe_directories(&self) -> broadcast::Receiver<ChangesWithMetadata<Directory>> {
        self.directory_tx.subscribe()
    }

    fn root_directory(&self) -> std::path::PathBuf {
        self.root_directory.clone()
    }
}

#[async_trait]
impl SyncableProvider for OrgModeSyncProvider {
    fn provider_name(&self) -> &str {
        "orgmode"
    }

    #[tracing::instrument(name = "provider.orgmode.sync", skip(self, _position))]
    async fn sync(&self, _position: StreamPosition) -> Result<StreamPosition> {
        use tracing::info;

        info!(
            "[OrgModeSyncProvider] Starting sync for directory: {}",
            self.root_directory.display()
        );

        // Check if directory exists
        if !self.fs.exists(&self.root_directory) {
            info!(
                "[OrgModeSyncProvider] WARNING: Root directory does not exist: {}",
                self.root_directory.display()
            );
        }

        // The position argument is ignored: a local filesystem walk is always
        // complete, so there is nothing to resume. The base for delta
        // computation (deletions, Created-vs-Updated) is the provider's own
        // persisted state -- loading it unconditionally is what makes external
        // deletions produce `Change::Deleted` instead of leaving stale rows.
        let _guard = self.sync_lock.lock().await;
        let old_state = self.load_state().await?;

        // Scan directory and compute changes
        let (new_state, dir_changes, file_changes) =
            self.scan_and_compute_changes(&old_state).await?;

        // Serialize new state for position
        let state_bytes = serde_json::to_vec(&new_state)
            .map_err(|e| format!("Failed to serialize sync state: {}", e))?;
        let new_position = StreamPosition::Version(state_bytes);

        let trace_context = holon_api::BatchTraceContext::from_current_span();

        // sync_token is None: the provider persists its own base directly
        // below instead of piggybacking it on the batch (the piggyback path
        // was never wired -- cache feeds apply batches with sync_token None).
        let dir_metadata = BatchMetadata {
            relation_name: "directory".to_string(),
            trace_context: trace_context.clone(),
            sync_token: None,
            seq: 0,
        };

        let file_metadata = BatchMetadata {
            relation_name: "file".to_string(),
            trace_context,
            sync_token: None,
            seq: 0,
        };

        info!(
            "[OrgModeSyncProvider] Emitting {} directory, {} file changes",
            dir_changes.len(),
            file_changes.len(),
        );

        let _ = self.directory_tx.send(WithMetadata {
            inner: dir_changes,
            metadata: dir_metadata,
        });

        let _ = self.file_tx.send(WithMetadata {
            inner: file_changes,
            metadata: file_metadata,
        });

        // Persist the base AFTER broadcasting, so a crash in between re-emits
        // (idempotent upserts / deletes downstream) rather than losing a delta.
        self.token_store
            .save_token(self.provider_name(), new_position.clone())
            .await?;

        Ok(new_position)
    }

    /// Optimized sync for post-operation changes
    ///
    /// IMPORTANT: Operations write files directly but don't return FieldDeltas
    /// yet (see TODO in OperationWrapper). Since operations already wrote
    /// files, we should NOT re-read and re-sync from files (causes duplicates).
    /// Instead, we just update the sync state hash to reflect that files are
    /// now in sync.
    ///
    /// Once operations return OperationResult with FieldDeltas, we can:
    /// 1. Extract file paths from FieldDeltas
    /// 2. Update sync state hash for those files
    /// 3. Optionally emit changes based on FieldDeltas (if needed for cache
    ///    updates)
    #[tracing::instrument(name = "provider.orgmode.sync_changes", skip(self, changes))]
    async fn sync_changes(&self, changes: &[FieldDelta]) -> Result<()> {
        use tracing::info;

        // TODO: Once operations return OperationResult with FieldDeltas, extract file
        // paths from changes For now, operations don't return FieldDeltas, so
        // changes is always empty Since operations write files directly, we
        // should NOT sync from files (would cause duplicates) Instead, we need
        // to update sync state hash for affected files

        if changes.is_empty() {
            // No FieldDeltas available - operations wrote files but didn't tell us which
            // ones We can't safely update sync state without knowing which
            // files changed For now, skip sync entirely - operations already
            // wrote files TODO: Once operations return FieldDeltas, extract
            // file paths and update sync state
            info!(
                "[OrgModeSyncProvider] sync_changes: No FieldDeltas available (operations don't \
                 return them yet), skipping sync to avoid duplicates"
            );
            return Ok(());
        }

        // Try to extract file paths from the changes
        let file_paths = self.extract_file_paths_from_deltas(changes);

        if file_paths.is_empty() {
            // FieldDeltas available but can't extract file paths
            // This shouldn't happen once FieldDeltas include file_path
            info!(
                "[OrgModeSyncProvider] sync_changes: FieldDeltas available but no file paths \
                 extracted"
            );
            return Ok(());
        }

        // Update sync state hash for affected files without emitting changes
        // (operations already updated database and wrote files)
        info!(
            "[OrgModeSyncProvider] sync_changes: Updating sync state for {} files",
            file_paths.len()
        );

        let _guard = self.sync_lock.lock().await;
        let old_state = self.load_state().await?;
        let mut new_state = old_state.clone();

        let canonical_root_sync = self
            .fs
            .canonicalize(&self.root_directory)
            .unwrap_or_else(|_| self.root_directory.clone());
        for file_path in file_paths {
            let canonical_fp = self
                .fs
                .canonicalize(&file_path)
                .unwrap_or_else(|_| file_path.clone());
            let file_id = generate_file_id(&canonical_fp, &canonical_root_sync).to_string();
            let content = self
                .fs
                .read_to_string(&file_path)
                .await
                .map_err(|e| format!("Failed to read file {}: {}", file_path.display(), e))?;
            let content_hash = compute_content_hash(&content);
            new_state.file_hashes.insert(file_id, content_hash);
        }

        let state_bytes = serde_json::to_vec(&new_state)
            .map_err(|e| format!("Failed to serialize sync state: {}", e))?;
        let new_position = StreamPosition::Version(state_bytes);

        self.token_store
            .save_token(self.provider_name(), new_position)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl OperationProvider for OrgModeSyncProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![generate_sync_operation(self.provider_name())]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        _: StorageEntity,
    ) -> Result<OperationResult> {
        let expected_entity_name = format!("{}.sync", self.provider_name());
        if entity_name != expected_entity_name.as_str() {
            return Err(format!(
                "Expected entity_name '{}', got '{}'",
                expected_entity_name, entity_name
            )
            .into());
        }

        if op_name != "sync" {
            return Err(format!("Expected op_name 'sync', got '{}'", op_name).into());
        }

        self.sync(StreamPosition::Beginning).await?;
        // Sync operations don't have FieldDeltas - they scan everything
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;

    use tempfile::tempdir;

    use super::*;

    /// Simple in-memory mock for SyncTokenStore
    struct MockSyncTokenStore {
        tokens: RwLock<HashMap<String, StreamPosition>>,
    }

    impl MockSyncTokenStore {
        fn new() -> Self {
            Self {
                tokens: RwLock::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl SyncTokenStore for MockSyncTokenStore {
        async fn load_token(&self, provider_name: &str) -> Result<Option<StreamPosition>> {
            Ok(self.tokens.read().unwrap().get(provider_name).cloned())
        }
        async fn save_token(&self, provider_name: &str, position: StreamPosition) -> Result<()> {
            self.tokens
                .write()
                .unwrap()
                .insert(provider_name.to_string(), position);
            Ok(())
        }
        async fn clear_all_tokens(&self) -> Result<()> {
            self.tokens.write().unwrap().clear();
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_sync_empty_directory() {
        let dir = tempdir().unwrap();
        let token_store = Arc::new(MockSyncTokenStore::new());
        let provider = OrgModeSyncProvider::new(
            dir.path().to_path_buf(),
            token_store,
            Arc::new(holon_filesystem::RealFileSystem),
        );

        let result = provider.sync(StreamPosition::Beginning).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_with_org_file() {
        let dir = tempdir().unwrap();
        let org_file = dir.path().join("test.org");
        std::fs::write(&org_file, "* Headline 1\n** Nested headline\n").unwrap();

        let token_store = Arc::new(MockSyncTokenStore::new());
        let provider = OrgModeSyncProvider::new(
            dir.path().to_path_buf(),
            token_store,
            Arc::new(holon_filesystem::RealFileSystem),
        );

        let mut file_rx = provider.subscribe_files();

        let result = provider.sync(StreamPosition::Beginning).await;
        assert!(result.is_ok());

        // Check that we received file changes
        let file_batch = file_rx.try_recv().unwrap();
        assert_eq!(file_batch.inner.len(), 1);

        // Blocks are no longer emitted by OrgModeSyncProvider — they go through
        // FileSyncController → command_bus → EventBus instead.
    }

    fn provider_for(dir: &std::path::Path) -> OrgModeSyncProvider {
        OrgModeSyncProvider::new(
            dir.to_path_buf(),
            Arc::new(MockSyncTokenStore::new()),
            Arc::new(holon_filesystem::RealFileSystem),
        )
    }

    /// Externally deleting a file must produce `Change::Deleted` on the next
    /// sync — regardless of the position the caller passes (all production
    /// callers pass `StreamPosition::Beginning`).
    #[tokio::test]
    async fn test_external_file_deletion_emits_deleted() {
        let dir = tempdir().unwrap();
        let org_file = dir.path().join("doomed.org");
        std::fs::write(&org_file, "* Headline\n").unwrap();

        let provider = provider_for(dir.path());
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let first = file_rx.try_recv().unwrap();
        assert_eq!(first.inner.len(), 1);
        let deleted_id = match &first.inner[0] {
            Change::Created { data, .. } => data.id.clone(),
            other => panic!("expected Created on first sync, got {:?}", other),
        };

        std::fs::remove_file(&org_file).unwrap();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let second = file_rx.try_recv().unwrap();
        assert_eq!(second.inner.len(), 1, "expected exactly the deletion");
        match &second.inner[0] {
            Change::Deleted { id, .. } => assert_eq!(id, &deleted_id),
            other => panic!("expected Deleted, got {:?}", other),
        }
    }

    /// Externally deleting a subdirectory must produce a directory
    /// `Change::Deleted` on the next sync.
    #[tokio::test]
    async fn test_external_directory_deletion_emits_deleted() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("notes");
        std::fs::create_dir(&sub).unwrap();

        let provider = provider_for(dir.path());
        let mut dir_rx = provider.subscribe_directories();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let first = dir_rx.try_recv().unwrap();
        assert_eq!(first.inner.len(), 1);

        std::fs::remove_dir(&sub).unwrap();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let second = dir_rx.try_recv().unwrap();
        assert_eq!(second.inner.len(), 1, "expected exactly the deletion");
        assert!(
            matches!(&second.inner[0], Change::Deleted { .. }),
            "expected Deleted, got {:?}",
            second.inner[0]
        );
    }

    /// Re-syncing an unchanged tree must emit NO changes — no spurious
    /// `Created` churn masked by cache upsert semantics.
    #[tokio::test]
    async fn test_resync_unchanged_tree_emits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.org"), "* A\n").unwrap();
        std::fs::write(dir.path().join("sub").join("b.org"), "* B\n").unwrap();

        let provider = provider_for(dir.path());
        let mut dir_rx = provider.subscribe_directories();
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        assert_eq!(dir_rx.try_recv().unwrap().inner.len(), 1);
        assert_eq!(file_rx.try_recv().unwrap().inner.len(), 2);

        provider.sync(StreamPosition::Beginning).await.unwrap();
        assert!(dir_rx.try_recv().unwrap().inner.is_empty());
        assert!(file_rx.try_recv().unwrap().inner.is_empty());
    }

    /// Modifying an already-known file must emit `Updated`, not `Created`.
    #[tokio::test]
    async fn test_modified_file_emits_updated() {
        let dir = tempdir().unwrap();
        let org_file = dir.path().join("live.org");
        std::fs::write(&org_file, "* v1\n").unwrap();

        let provider = provider_for(dir.path());
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        assert!(matches!(
            &file_rx.try_recv().unwrap().inner[0],
            Change::Created { .. }
        ));

        std::fs::write(&org_file, "* v2\n").unwrap();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let batch = file_rx.try_recv().unwrap();
        assert_eq!(batch.inner.len(), 1);
        assert!(
            matches!(&batch.inner[0], Change::Updated { .. }),
            "expected Updated, got {:?}",
            batch.inner[0]
        );
    }
}
