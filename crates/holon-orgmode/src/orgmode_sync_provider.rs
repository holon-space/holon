//! Stream-based OrgModeSyncProvider
//!
//! This sync provider scans an org-mode directory and emits changes on typed
//! streams. Architecture:
//! - ONE sync() call → the file change stream
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
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_core::generate_sync_operation;
use holon_core::storage::types::StorageEntity;
use holon_filesystem::File;
use holon_filesystem::FileSystem;
use holon_filesystem::file::ChangesWithMetadata;
use tokio::sync::broadcast;

use crate::parser::compute_content_hash;
use crate::parser::generate_file_id;

/// `File::parent_id` value for a file sitting directly in the vault root.
/// A plain relative path, not an entity id — see `File::parent_id`.
const ROOT_PARENT: &str = ".";

/// Sync state stored as JSON in token store.
///
/// `deny_unknown_fields` is deliberate: this struct IS the on-disk token
/// format, and silently ignoring a field we no longer understand is exactly
/// the kind of format drift that hides bugs. Unknown fields route to the
/// explicit legacy migration in `load_state` instead.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SyncState {
    /// Map of file paths to their content hashes
    file_hashes: HashMap<String, String>,
}

/// Pre-`Directory`-purge token shape. Vaults synced before the `Directory`
/// entity was deleted carry a `known_dirs` map that no longer has any meaning.
/// Parsed only to salvage `file_hashes`, so an upgrade does not re-emit every
/// file as `Created`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySyncState {
    file_hashes: HashMap<String, String>,
    #[allow(dead_code)]
    known_dirs: HashMap<String, bool>,
}

/// Stream-based OrgModeSyncProvider that scans directories and emits changes on
/// typed streams
pub struct OrgModeSyncProvider {
    root_directory: PathBuf,
    token_store: Arc<dyn SyncTokenStore>,
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
            file_tx: broadcast::channel(1000).0,
            fs,
            sync_lock: tokio::sync::Mutex::new(()),
        }
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
            StreamPosition::Version(bytes) => match serde_json::from_slice::<SyncState>(&bytes) {
                Ok(state) => Ok(state),
                Err(current_err) => {
                    // Not the current shape. Try the one legacy shape we know
                    // how to migrate, and say so loudly — a token we cannot
                    // account for is an error, never a silent reset to
                    // `default()` (that would re-emit the whole vault as
                    // `Created` while looking perfectly healthy).
                    let legacy: LegacySyncState =
                        serde_json::from_slice(&bytes).map_err(|legacy_err| {
                            format!(
                                "Failed to parse orgmode sync state. As current format: \
                                 {current_err}. As pre-Directory-purge legacy format: \
                                 {legacy_err}"
                            )
                        })?;
                    tracing::warn!(
                        file_hashes = legacy.file_hashes.len(),
                        "[OrgModeSyncProvider] MIGRATING sync token: dropping the obsolete \
                         `known_dirs` field left by the deleted Directory entity. File hashes \
                         are carried over, so this does not re-scan the vault. This warning \
                         should appear exactly once per vault."
                    );
                    Ok(SyncState {
                        file_hashes: legacy.file_hashes,
                    })
                }
            },
        }
    }

    /// Perform directory scan and compute changes
    async fn scan_and_compute_changes(
        &self,
        old_state: &SyncState,
    ) -> Result<(SyncState, Vec<Change<File>>)> {
        let origin = ChangeOrigin::remote_with_current_span();
        let mut new_state = SyncState::default();
        let mut file_changes = Vec::new();

        // Track what we've seen to detect deletions
        let mut seen_files: HashMap<String, bool> = HashMap::new();

        let scanned = crate::file_watcher::scan_directory(self.fs.as_ref(), &self.root_directory)
            .await
            .map_err(|e| format!("Failed to scan {}: {e}", self.root_directory.display()))?;

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
                // Relative path of the containing folder — a plain path
                // string, never an entity id (see `File::parent_id`).
                let parent_id = match path.parent() {
                    Some(p) if p != self.root_directory => {
                        // `path` came out of a scan rooted at `root_directory`,
                        // so its parent is always under the root. A failure
                        // here means the scanner returned a foreign path.
                        let rel = p.strip_prefix(&self.root_directory).map_err(|e| {
                            format!(
                                "Scanned file parent {} is not under vault root {}: {e}",
                                p.display(),
                                self.root_directory.display()
                            )
                        })?;
                        rel.to_string_lossy().to_string()
                    }
                    _ => ROOT_PARENT.to_string(),
                };

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
            "[OrgModeSyncProvider] Scan complete: {} files found",
            scanned.files.len()
        );

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

        Ok((new_state, file_changes))
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

#[async_trait]
impl SyncableProvider for OrgModeSyncProvider {
    fn provider_name(&self) -> &str {
        "orgmode"
    }

    /// The position argument is unused: `SyncableProvider::sync` fixes this
    /// signature, but a local filesystem walk is always complete, so there is
    /// no cursor to resume from. The delta base is the provider's own
    /// persisted `SyncState` (see module header).
    #[tracing::instrument(name = "provider.orgmode.sync", skip(self))]
    async fn sync(&self, _: StreamPosition) -> Result<StreamPosition> {
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
        let (new_state, file_changes) = self.scan_and_compute_changes(&old_state).await?;

        // Serialize new state for position
        let state_bytes = serde_json::to_vec(&new_state)
            .map_err(|e| format!("Failed to serialize sync state: {}", e))?;
        let new_position = StreamPosition::Version(state_bytes);

        let trace_context = holon_api::BatchTraceContext::from_current_span();

        // sync_token is None: the provider persists its own base directly
        // below instead of piggybacking it on the batch (the piggyback path
        // was never wired -- cache feeds apply batches with sync_token None).
        let file_metadata = BatchMetadata {
            relation_name: "file".to_string(),
            trace_context,
            linked_contexts: Vec::new(),
            sync_token: None,
            seq: 0,
            degraded: None,
        };

        info!(
            "[OrgModeSyncProvider] Emitting {} file changes",
            file_changes.len(),
        );

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

    /// A sync token written before the `Directory` purge still carries
    /// `known_dirs`. It must migrate — keeping `file_hashes` so the vault is
    /// not re-emitted as `Created` — rather than being silently reset.
    #[tokio::test]
    async fn test_legacy_sync_token_migrates_keeping_file_hashes() {
        let dir = tempdir().unwrap();
        let provider = provider_for(dir.path());

        let legacy = br#"{"file_hashes":{"file:a.org":"deadbeef"},"known_dirs":{"Notes":true}}"#;
        provider
            .token_store
            .save_token("orgmode", StreamPosition::Version(legacy.to_vec()))
            .await
            .unwrap();

        let state = provider.load_state().await.unwrap();
        assert_eq!(state.file_hashes.get("file:a.org").unwrap(), "deadbeef");
    }

    /// A token that matches neither the current nor the known legacy shape is
    /// an error, never a silent reset to `default()`.
    #[tokio::test]
    async fn test_unrecognized_sync_token_errors_loudly() {
        let dir = tempdir().unwrap();
        let provider = provider_for(dir.path());

        provider
            .token_store
            .save_token(
                "orgmode",
                StreamPosition::Version(br#"{"totally":"unexpected"}"#.to_vec()),
            )
            .await
            .unwrap();

        let err = provider.load_state().await.unwrap_err().to_string();
        assert!(
            err.contains("Failed to parse orgmode sync state"),
            "error must name the failure and both attempted shapes, got: {err}"
        );
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

    /// A folder whose name contains a space (e.g. `Agentic DPL`) must sync
    /// without panicking. This is the boot-crash repro: the scan path feeds a
    /// raw relative path into an id that was parsed as an RFC 3986 URI, and a
    /// space is not a legal URI character.
    #[tokio::test]
    async fn test_directory_name_with_space_syncs() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("Agentic DPL");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("note.org"), "* Note\n").unwrap();

        let provider = provider_for(dir.path());
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();

        let batch = file_rx.try_recv().unwrap();
        assert_eq!(batch.inner.len(), 1, "expected the one .org file");
        match &batch.inner[0] {
            Change::Created { data, .. } => {
                assert_eq!(data.parent_id, "Agentic DPL");
            }
            other => panic!("expected Created, got {:?}", other),
        }
    }

    /// Externally deleting a subdirectory must produce a `Change::Deleted` for
    /// the files it contained. Folders themselves are not entities; the files
    /// inside them are what the rest of the system tracks.
    #[tokio::test]
    async fn test_external_directory_deletion_emits_file_deletions() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("notes");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("kept.org"), "* Kept\n").unwrap();

        let provider = provider_for(dir.path());
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let first = file_rx.try_recv().unwrap();
        assert_eq!(first.inner.len(), 1);
        let created_id = match &first.inner[0] {
            Change::Created { data, .. } => data.id.clone(),
            other => panic!("expected Created, got {:?}", other),
        };

        std::fs::remove_dir_all(&sub).unwrap();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        let second = file_rx.try_recv().unwrap();
        assert_eq!(second.inner.len(), 1, "expected exactly the deletion");
        match &second.inner[0] {
            Change::Deleted { id, .. } => assert_eq!(id, &created_id),
            other => panic!("expected Deleted, got {:?}", other),
        }
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
        let mut file_rx = provider.subscribe_files();

        provider.sync(StreamPosition::Beginning).await.unwrap();
        assert_eq!(file_rx.try_recv().unwrap().inner.len(), 2);

        provider.sync(StreamPosition::Beginning).await.unwrap();
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
