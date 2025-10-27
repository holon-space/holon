//! Dependency Injection module for OrgMode integration
//!
//! This module provides DI registration for OrgMode-specific services using fluxdi.
//! OrgMode is now independent of Loro — it will use LoroBlockOperations if available in DI,
//! otherwise falls back to SqlOperationProvider for direct database writes.
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_orgmode::di::OrgModeInjectorExt;
//! use std::path::PathBuf;
//!
//! services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};

use holon_filesystem::directory::Directory;
use holon_filesystem::File;

use crate::file_watcher::OrgFileWatcher;
use crate::org_renderer::OrgRenderer;
use crate::org_sync_controller::OrgSyncController;
use crate::orgmode_event_adapter::OrgModeEventAdapter;
use crate::traits::{BlockReader, DocumentManager};
use crate::OrgModeSyncProvider;
use holon::core::datasource::{OperationProvider, SyncTokenStore, SyncableProvider};
use holon::core::operation_wrapper::OperationWrapper;
use holon::core::queryable_cache::QueryableCache;
use holon::sync::event_bus::{EventBus, PublishErrorTracker};
use holon::sync::{LoroBlockOperations, LoroDocumentStore, TursoEventBus};
use holon::type_registry::TypeRegistry;
use holon_api::block::Block;
use holon_api::{EntityName, EntityUri};

/// Signal that indicates the FileWatcher is ready to receive file change events.
///
/// Tests can wait on this signal to ensure the file watcher is established
/// before making external file modifications.
#[derive(Clone)]
pub struct FileWatcherReadySignal {
    receiver: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
}

impl FileWatcherReadySignal {
    /// Create a new ready signal (sender/receiver pair)
    pub fn new() -> (FileWatcherReadySender, Self) {
        let (tx, rx) = tokio::sync::watch::channel(None);
        (FileWatcherReadySender { sender: tx }, Self { receiver: rx })
    }

    /// Check if startup has completed (either success or failure).
    pub fn is_completed(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    /// Wait until the file watcher signals readiness.
    ///
    /// Returns `Ok(())` on success, `Err` if the OrgSyncController startup failed.
    /// Errors are propagated — never swallowed.
    pub async fn wait_ready(&self) -> anyhow::Result<()> {
        let mut receiver = self.receiver.clone();
        // Wait until the value is Some(_)
        let result = receiver.wait_for(|v| v.is_some()).await.map_err(|_| {
            anyhow::anyhow!("FileWatcherReadySignal sender dropped without signaling")
        })?;
        match result.as_ref().unwrap() {
            Ok(()) => Ok(()),
            Err(msg) => Err(anyhow::anyhow!("OrgSyncController startup failed: {}", msg)),
        }
    }
}

/// Sender half of the FileWatcher ready signal
pub struct FileWatcherReadySender {
    sender: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl FileWatcherReadySender {
    /// Signal successful readiness.
    pub fn signal_ready(self) {
        let _ = self.sender.send(Some(Ok(())));
    }

    /// Signal that startup failed. The error message propagates to the waiter.
    pub fn signal_error(self, error: String) {
        let _ = self.sender.send(Some(Err(error)));
    }
}

/// Event-driven idle signal for the OrgSyncController loop.
///
/// The controller's background task calls [`mark_progress`] after each
/// iteration where it actually processed an event (file change or block
/// change). Tests use [`wait_quiescent`] to wait until the loop has had no
/// activity for a short window — proving that all org-file writes triggered
/// by recent SQL mutations have already landed on disk.
///
/// This replaces filesystem mtime polling on the hot path (~30 ms per call)
/// with an event signal that completes in ~1 ms when the loop is genuinely
/// idle. Callers that don't have access to the signal (or want extra safety)
/// fall back to mtime polling.
///
/// [`mark_progress`]: OrgSyncIdleSignal::mark_progress
/// [`wait_quiescent`]: OrgSyncIdleSignal::wait_quiescent
#[derive(Debug)]
pub struct OrgSyncIdleSignal {
    /// Monotonic count of completed loop iterations. Bumped after every
    /// processed event (file or block change).
    tick: std::sync::atomic::AtomicU64,
    /// Wakes any task waiting in [`wait_quiescent`] whenever the tick advances.
    notify: tokio::sync::Notify,
}

impl OrgSyncIdleSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tick: std::sync::atomic::AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
        })
    }

    /// Current tick value. Increases monotonically.
    pub fn current_tick(&self) -> u64 {
        self.tick.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Called by the controller loop after each processed event.
    pub fn mark_progress(&self) {
        self.tick.fetch_add(1, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Wait until the controller loop has been idle (no [`mark_progress`]
    /// call) for `quiescence`, or `timeout` elapses. Returns `true` if
    /// quiescence was reached, `false` on timeout.
    ///
    /// Cost when already idle: one `tokio::time::timeout` of `quiescence`.
    /// Cost when busy: as long as it takes for the loop to drain, capped by
    /// `timeout`.
    ///
    /// [`mark_progress`]: Self::mark_progress
    pub async fn wait_quiescent(
        &self,
        quiescence: std::time::Duration,
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.current_tick();
            // Subscribe BEFORE re-reading the tick to avoid missing a wake.
            let notified = self.notify.notified();
            if self.current_tick() != snapshot {
                // Activity already happened; loop again.
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let wait = quiescence.min(remaining);
            match tokio::time::timeout(wait, notified).await {
                Err(_) => {
                    // No notification within `quiescence` — the loop is idle.
                    if self.current_tick() == snapshot {
                        return true;
                    }
                    // A wake landed between the timeout firing and the re-check;
                    // treat it as activity and loop.
                }
                Ok(()) => {
                    // Got woken — keep waiting unless we ran out of time.
                    if tokio::time::Instant::now() >= deadline {
                        return false;
                    }
                }
            }
        }
    }
}

/// Scan a directory recursively for .org files.
///
/// Delegates to `file_watcher::scan_directory` — the single source of truth
/// for directory walking (respects .gitignore, skips .git/.jj).
fn scan_org_files(dir: &std::path::Path) -> Vec<PathBuf> {
    crate::file_watcher::scan_directory(dir).files
}

// =============================================================================
// Trait implementations for decoupling org-mode from Loro/Turso
// =============================================================================

/// BlockReader backed by QueryableCache<Block>.
///
/// Uses the existing DataSource abstraction instead of raw SQL.
/// All block reads go through a single `get_all()` call + in-memory filtering.
pub struct CacheBlockReader {
    cache: Arc<QueryableCache<Block>>,
}

impl CacheBlockReader {
    pub fn new(cache: Arc<QueryableCache<Block>>) -> Self {
        Self { cache }
    }
}

#[async_trait::async_trait]
impl BlockReader for CacheBlockReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        use holon::core::datasource::DataSource;
        use std::collections::HashSet;

        let all_blocks = self
            .cache
            .get_all()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // BFS to collect all descendants of doc_id
        let mut result = Vec::new();
        let mut frontier: HashSet<&str> = HashSet::new();
        frontier.insert(doc_id.as_str());

        loop {
            let mut next_frontier = HashSet::new();
            let mut found_any = false;
            for block in &all_blocks {
                if frontier.contains(block.parent_id.as_str())
                    && !result.iter().any(|b: &Block| b.id == block.id)
                {
                    if block.is_document() {
                        continue;
                    }
                    next_frontier.insert(block.id.as_str());
                    result.push(block.clone());
                    found_any = true;
                }
            }
            if !found_any {
                break;
            }
            frontier = next_frontier;
        }

        Ok(result)
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        use holon::core::datasource::DataSource;
        use std::collections::HashMap;

        let all_blocks = self
            .cache
            .get_all()
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader] Failed to load blocks: {e}"))?;

        let mut children_of: HashMap<EntityUri, Vec<&Block>> = HashMap::new();
        for block in &all_blocks {
            children_of
                .entry(block.parent_id.clone())
                .or_default()
                .push(block);
        }

        let doc_uris: Vec<EntityUri> = children_of
            .keys()
            .filter(|pid| pid.is_no_parent() || pid.is_sentinel())
            .cloned()
            .collect();

        let mut result = Vec::new();
        for doc_uri in doc_uris {
            let mut blocks = Vec::new();
            let mut queue: Vec<EntityUri> = vec![doc_uri.clone()];
            while let Some(pid) = queue.pop() {
                if let Some(children) = children_of.get(&pid) {
                    for block in children {
                        if block.is_document() {
                            continue;
                        }
                        queue.push(block.id.clone());
                        blocks.push((*block).clone());
                    }
                }
            }
            if !blocks.is_empty() {
                result.push((doc_uri, blocks));
            }
        }

        Ok(result)
    }
}

/// DocumentManager backed by CDC-driven LiveData over document blocks.
///
/// All reads (`find_by_parent_and_name`, `get_by_id`) are in-memory lookups
/// against a `LiveData<Block>` that stays current via a Turso materialized
/// view CDC stream over blocks where `name IS NOT NULL`.
/// Writes go through `SqlOperationProvider` (SQL); the matview
/// CDC automatically propagates them into the LiveData.
pub struct LiveDocumentManager {
    live: Arc<holon::sync::LiveData<Block>>,
    command_bus: Arc<dyn OperationProvider>,
}

impl LiveDocumentManager {
    /// Create a LiveDocumentManager backed by a materialized view over document blocks.
    pub async fn new(
        command_bus: Arc<dyn OperationProvider>,
        backend: Arc<tokio::sync::RwLock<holon::storage::turso::TursoBackend>>,
    ) -> anyhow::Result<Self> {
        let backend_guard = backend.read().await;
        let db_handle = backend_guard.handle();
        drop(backend_guard);

        let matview_mgr =
            holon::sync::MatviewManager::new(db_handle, Arc::new(tokio::sync::Mutex::new(())));

        let result = matview_mgr
            .watch("SELECT * FROM block WHERE name IS NOT NULL")
            .await?;

        let live = holon::sync::LiveData::new(
            result.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("document block row missing 'id'"))
            },
            |row| Block::try_from(row.clone()).map_err(|e| anyhow::anyhow!("{}", e)),
        );
        live.subscribe(result.stream);

        tracing::info!(
            "[LiveDocumentManager] Watching {} document blocks via matview",
            live.read().len()
        );

        Ok(Self { live, command_bus })
    }
}

#[async_trait::async_trait]
impl DocumentManager for LiveDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        name: &str,
    ) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs
            .values()
            .find(|d| d.parent_id == *parent_id && d.name.as_deref() == Some(name))
            .cloned())
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        use crate::block_params::build_block_params;
        // Route document creation events to the document's own ID.
        // _routing_doc_uri is only event routing metadata (not stored in DB) —
        // it tells OrgSyncController which file to re-render.
        let params = build_block_params(&doc, &doc.parent_id, &doc.id);
        // INSERT OR IGNORE: if a document with the same (parent_id, name) already
        // exists (UNIQUE index), the INSERT is silently skipped.
        let result = self
            .command_bus
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // If the response carries an existing id, the INSERT was ignored —
        // a document with the same (parent_id, name) already exists in the DB.
        // Return that existing document instead of the one we tried to insert.
        if let Some(holon_api::Value::String(existing_id)) = result.response {
            tracing::debug!(
                "[LiveDocumentManager] Document {:?} already exists as {} (attempted id={})",
                doc.name,
                existing_id,
                doc.id,
            );
            let existing_uri = EntityUri::from_raw(&existing_id);
            if let Some(existing) = self.get_by_id(&existing_uri).await? {
                return Ok(existing);
            }
            // The document exists in SQL but not in LiveData.
            // Insert it so subsequent find_by_parent_and_name / get_by_id lookups succeed.
            let mut existing_doc = doc.clone();
            existing_doc.id = existing_uri;
            self.live
                .insert(existing_doc.id.as_str().to_string(), existing_doc.clone());
            return Ok(existing_doc);
        }

        self.live.insert(doc.id.as_str().to_string(), doc.clone());
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs.get(id.as_str()).cloned())
    }

    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        use crate::block_params::build_block_params;
        let params = build_block_params(doc, &doc.parent_id, &doc.id);
        self.command_bus
            .execute_operation(&EntityName::new("block"), "update", params)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        // Update in-memory cache
        self.live.insert(doc.id.as_str().to_string(), doc.clone());
        Ok(())
    }
}

/// AliasRegistrar backed by LoroDocumentStore.
///
/// Must share the same `Arc<RwLock<LoroDocumentStore>>` as LoroBlockReader/LoroBlockOperations.
pub struct LoroAliasRegistrar {
    doc_store: Arc<tokio::sync::RwLock<LoroDocumentStore>>,
}

#[async_trait::async_trait]
impl crate::org_sync_controller::AliasRegistrar for LoroAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        let store = self.doc_store.read().await;
        store.register_alias(doc_id.as_str(), path).await;
    }

    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        let store = self.doc_store.read().await;
        store.resolve_alias_to_path(doc_id.as_str()).await
    }
}

/// Configuration for OrgMode integration
#[derive(Clone, Debug)]
pub struct OrgModeConfig {
    /// Root directory containing .org files
    pub root_directory: PathBuf,
    /// Directory where .loro files are stored (legacy, used when Loro is managed by OrgMode)
    pub loro_storage_dir: PathBuf,
    /// Debounce window in milliseconds for OrgSyncController.
    /// Events are batched and rendered after this quiet period.
    pub debounce_ms: u64,
    /// Shell command to run after each org file write (e.g. "jj new").
    /// Runs in root_directory with HOLON_FILE env var set to the written path.
    pub post_org_write_hook: Option<String>,
}

impl OrgModeConfig {
    pub fn new(root_directory: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        // This ensures path comparisons work correctly when file watcher reports
        // canonicalized paths
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        let loro_storage_dir = root_directory.join(".loro");
        Self {
            root_directory,
            loro_storage_dir,
            debounce_ms: 500,
            post_org_write_hook: None,
        }
    }

    pub fn with_loro_storage(root_directory: PathBuf, loro_storage_dir: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        let loro_storage_dir = std::fs::canonicalize(&loro_storage_dir).unwrap_or(loro_storage_dir);
        Self {
            root_directory,
            loro_storage_dir,
            debounce_ms: 500,
            post_org_write_hook: None,
        }
    }
}

/// ServiceModule for OrgMode integration
///
/// Registers OrgMode-specific services in the DI container.
/// Loro services are NOT registered here — they come from LoroModule (if enabled).
///
/// OrgMode will detect if LoroBlockOperations is available in DI and use it;
/// otherwise it falls back to SqlOperationProvider.
pub struct OrgModeModule;

impl Module for OrgModeModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        use tracing::{error, info};

        info!("[OrgModeModule] register_services called");

        // Create and register FileWatcherReadySignal
        // Tests can wait on this to ensure file watcher is ready before external mutations
        let (ready_sender, ready_signal) = FileWatcherReadySignal::new();
        let ready_signal = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_signal)));
        injector.provide::<FileWatcherReadySignal>(Provider::root(move |_| {
            let signal = ready_signal
                .lock()
                .unwrap()
                .take()
                .expect("FileWatcherReadySignal factory called twice");
            Shared::new(signal)
        }));
        // Store sender in Arc<Mutex> so we can move it into the spawned task later
        let ready_sender = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_sender)));
        let ready_sender_for_factory = ready_sender.clone();

        // Create and register OrgSyncIdleSignal
        // Tests use this to skip mtime polling on the hot path.
        let idle_signal = OrgSyncIdleSignal::new();
        let idle_signal_for_factory = idle_signal.clone();
        injector
            .provide::<OrgSyncIdleSignal>(Provider::root(move |_| idle_signal_for_factory.clone()));
        let idle_signal_for_loop = idle_signal;

        // Register OrgModeSyncProvider as a factory
        injector.provide::<OrgModeSyncProvider>(Provider::root_async(async |resolver| {
            let config = resolver.resolve::<OrgModeConfig>();
            let token_store = resolver
                .try_resolve_async::<dyn SyncTokenStore>()
                .await
                .expect("[OrgModeModule] SyncTokenStore not found in DI");
            Shared::new(OrgModeSyncProvider::new(
                config.root_directory.clone(),
                token_store,
            ))
        }));

        // Register SyncableProvider trait implementation
        injector.provide_into_set::<dyn SyncableProvider>(Provider::root(|resolver| {
            let sync_provider = resolver.resolve::<OrgModeSyncProvider>();
            sync_provider.clone() as Arc<dyn SyncableProvider>
        }));

        // Register filesystem entity types in the TypeRegistry for GQL graph.
        // Done inside an async provider so TypeRegistry is already available.
        injector.provide::<QueryableCache<Directory>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(Directory::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register Directory type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));
        injector.provide::<QueryableCache<File>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(File::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register File type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));

        // TursoEventBus is registered by FrontendConfig shared infrastructure

        // Register OrgRenderer
        injector.provide::<OrgRenderer>(Provider::root(|_resolver| Shared::new(OrgRenderer)));

        // Set up event bus wiring and background tasks.
        // This factory resolves LoroBlockOperations if available (Loro enabled),
        // otherwise creates a SqlOperationProvider for direct SQL block operations.
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(move |resolver| {
            let ready_sender_clone = ready_sender_for_factory.clone();
            let idle_signal_clone = idle_signal_for_loop.clone();
            async move {
            // ============================================================
            // PHASE 1: Resolve ALL services that run DDL
            // This ensures all schema initialization completes BEFORE
            // any background tasks start using the database.
            // ============================================================
            info!("[OrgMode] Phase 1: Resolving services (DDL)");

            let _dir_cache = resolver.resolve_async::<QueryableCache<Directory>>().await;
            let _file_cache = resolver.resolve_async::<QueryableCache<File>>().await;
            let _block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
            let sync_provider = resolver.resolve_async::<OrgModeSyncProvider>().await;

            // IMPORTANT: Resolve TursoEventBus HERE, not after spawns!
            // TursoEventBus::init_schema() runs DDL that must complete
            // before any background tasks use the database.
            let event_bus = resolver.resolve_async::<TursoEventBus>().await;
            let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();

            // Resolve remaining services
            let config = resolver.resolve::<OrgModeConfig>();

            // Try to resolve Loro services (available if LoroModule was registered)
            // ALLOW(ok): optional DI service
            let loro_ops: Option<Arc<LoroBlockOperations>> =
                resolver.try_resolve::<LoroBlockOperations>().ok();

            let loro_available = loro_ops.is_some();
            info!(
                "[OrgMode] Phase 1 complete: All DDL finished (loro={})",
                loro_available
            );

            // Resolve DbHandle unconditionally — Turso is always available
            let db_handle_provider =
                resolver.resolve::<dyn holon::di::DbHandleProvider>();
            let db_handle = db_handle_provider.handle();

            // OrgSyncController writes through SQL ops; CacheBlockReader reads from QueryableCache
            // which is also backed by the same Turso database, ensuring consistency.
            let sql_ops = Arc::new(holon::core::SqlOperationProvider::with_event_bus(
                db_handle.clone(),
                "block".to_string(),
                "block".to_string(),
                "block".to_string(),
                event_bus_arc.clone(),
            ));

            let command_bus: Arc<dyn OperationProvider> =
                sql_ops.clone() as Arc<dyn OperationProvider>;

            // ============================================================
            // PHASE 2: Create OrgSyncController
            // Single controller using last_projection for echo suppression.
            // ============================================================
            info!("[OrgMode] Phase 2: Creating OrgSyncController");

            info!("[OrgMode] Phase 2 complete");

            // ============================================================
            // PHASE 3: Spawn background tasks
            // The DatabaseActor serializes all operations, eliminating race conditions
            // between DDL and DML operations.
            // ============================================================
            info!("[OrgMode] Phase 3: Spawning background tasks");

            // NOTE: Direct cache writes (Task 1) removed. All block writes now go
            // through EventBus (via OrgSyncController → command_bus → EventBus).
            // Directory and file changes still go through OrgModeEventAdapter → EventBus.

            // Initial sync task
            // The DatabaseActor serializes all operations, eliminating race conditions.
            {
                let sync_provider_clone = sync_provider.clone();
                tokio::spawn(async move {
                    use holon::core::datasource::SyncableProvider;
                    if let Err(e) = sync_provider_clone
                        .sync(holon::core::datasource::StreamPosition::Beginning)
                        .await
                    {
                        error!("[OrgMode] Initial sync failed: {}", e);
                    }
                });
            }

            // Loro ↔ command/event bus is wired by `LoroModule` via
            // `LoroSyncControllerHandle`; see `crates/holon/src/sync/loro_module.rs`.

            // OrgModeSyncProvider → EventBus (directories and files only)
            {
                let sync_provider_clone = sync_provider.clone();
                let event_bus_clone = event_bus_arc.clone();
                let error_tracker = resolver.try_resolve::<PublishErrorTracker>()
                    .map(|t| (*t).clone())
                    .unwrap_or_else(|_| PublishErrorTracker::new());
                tokio::spawn(async move {
                    let adapter =
                        OrgModeEventAdapter::with_error_tracker(event_bus_clone, error_tracker);
                    let dir_rx = sync_provider_clone.subscribe_directories();
                    let file_rx = sync_provider_clone.subscribe_files();
                    if let Err(e) = adapter.start(dir_rx, file_rx) {
                        error!("[OrgMode] Failed to start OrgModeEventAdapter: {}", e);
                    }
                });
            }

            // OrgSyncController: unified file ↔ block sync
            // Subscribes to EventBus for block events (works with both Loro and SQL paths).
            // Runs on a single task via tokio::select!, serializing
            // on_file_changed and on_block_changed — no locks needed.
            {
                let command_bus = command_bus.clone();
                let config_clone = config.clone();
                let event_bus_for_ctrl = event_bus_arc.clone();
                let ready_sender_clone = ready_sender_clone.clone();
                let idle_signal_for_task = idle_signal_clone.clone();

                let loro_ops_clone = loro_ops.clone();
                let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
                let backend_provider =
                    resolver.resolve::<dyn holon::di::TursoBackendProvider>();
                let backend_for_live_docs = backend_provider.backend();
                let command_bus_for_docs = command_bus.clone();

                tokio::spawn(async move {
                    let doc_manager: Arc<dyn DocumentManager> = Arc::new(
                        LiveDocumentManager::new(command_bus_for_docs, backend_for_live_docs)
                            .await
                            .expect("Failed to create LiveDocumentManager"),
                    );

                    let block_reader: Arc<dyn BlockReader> =
                        Arc::new(CacheBlockReader::new(block_cache));

                    let mut controller = OrgSyncController::new(
                        block_reader,
                        command_bus,
                        doc_manager,
                        config_clone.root_directory.clone(),
                    );

                    if let Some(hook_cmd) = config_clone.post_org_write_hook.clone() {
                        controller = controller.with_post_org_write_hook(hook_cmd);
                    }

                    // Wire alias registrar when Loro is available (UUID→path mapping)
                    if let Some(ref ops) = loro_ops_clone {
                        let shared_doc_store = ops.shared_doc_store();
                        let alias_registrar: Arc<dyn crate::org_sync_controller::AliasRegistrar> =
                            Arc::new(LoroAliasRegistrar { doc_store: shared_doc_store });
                        controller = controller.with_alias_registrar(alias_registrar);
                    }

                    if let Err(e) = controller.initialize().await {
                        let msg = format!("OrgSyncController initialization failed: {}", e);
                        error!("[OrgMode] {}", msg);
                        if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                            sender.signal_error(msg);
                        }
                        return;
                    }

                    // Subscribe to EventBus for block events
                    let block_filter = holon::sync::event_bus::EventFilter::new()
                        .with_aggregate_type(holon::sync::event_bus::AggregateType::Block)
                        .with_status(holon::sync::event_bus::EventStatus::Confirmed);
                    let mut event_rx = match event_bus_for_ctrl.subscribe(block_filter).await {
                        Ok(rx) => rx,
                        Err(e) => {
                            let msg = format!("Failed to subscribe to EventBus: {}", e);
                            error!("[OrgMode] {}", msg);
                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_error(msg);
                            }
                            return;
                        }
                    };

                    match OrgFileWatcher::new(&config_clone.root_directory) {
                        Ok(watcher) => {
                            let (_watcher, mut file_rx, _) = watcher.into_parts();
                            info!(
                                "[OrgMode] File watcher started for: {}",
                                config_clone.root_directory.display()
                            );

                            // Process existing org files BEFORE signaling ready
                            let org_files = scan_org_files(&config_clone.root_directory);
                            for file_path in org_files {
                                if let Err(e) =
                                    controller.on_file_changed(&file_path).await
                                {
                                    error!(
                                        "[OrgMode] Failed to process existing file {}: {}",
                                        file_path.display(),
                                        e
                                    );
                                }
                            }

                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_ready();
                            }


                            // Main loop: handle file changes and EventBus block events.
                            //
                            // A periodic `poll_tick` backstops the notify-driven
                            // `file_rx` path. FSEvents on macOS can coalesce or
                            // drop events under load, which would otherwise leave
                            // externally-edited files unprocessed (SQL stays at
                            // old content, files disagree with DB). The poll
                            // scans `last_projection` for disk/projection
                            // mismatches and ingests them exactly like a
                            // file-watcher delivery — so a missed FSEvent becomes
                            // a latency blip rather than a correctness hole.
                            let mut poll_tick = tokio::time::interval(
                                tokio::time::Duration::from_millis(100),
                            );
                            poll_tick.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Skip,
                            );
                            loop {
                                tokio::select! {
                                    Some(file_path) = file_rx.recv() => {
                                        eprintln!("[ORGSYNC_TRACE] file_rx -> on_file_changed({})", file_path.display());
                                        if let Err(e) = controller.on_file_changed(&file_path).await {
                                            eprintln!(
                                                "[ORGSYNC_TRACE] on_file_changed ERROR for {}: {}",
                                                file_path.display(), e
                                            );
                                            error!(
                                                "[OrgMode] File change error {}: {}",
                                                file_path.display(), e
                                            );
                                        } else {
                                            eprintln!("[ORGSYNC_TRACE] on_file_changed OK for {}", file_path.display());
                                        }
                                        idle_signal_for_task.mark_progress();
                                    }
                                    _ = poll_tick.tick() => {
                                        match controller.poll_external_changes().await {
                                            Ok(n) if n > 0 => {
                                                eprintln!("[ORGSYNC_TRACE] poll ingested {} file(s)", n);
                                                idle_signal_for_task.mark_progress();
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                eprintln!("[ORGSYNC_TRACE] poll ERROR: {}", e);
                                                error!("[OrgMode] poll_external_changes error: {}", e);
                                            }
                                        }
                                    }
                                    Some(event) = tokio_stream::StreamExt::next(&mut event_rx) => {
                                        let event_id = event.id.clone();
                                        let doc_ids = extract_doc_ids_from_event(&event);
                                        if doc_ids.is_empty() {
                                            // No routing info — fall back to re-rendering
                                            // all tracked files instead of dropping the event.
                                            info!(
                                                "[OrgMode] Block event {} ({:?}) missing _routing_doc_uri — re-rendering all tracked files",
                                                event.aggregate_id, event.event_kind,
                                            );
                                            if let Err(e) = controller.re_render_all_tracked().await {
                                                error!("[OrgMode] re_render_all_tracked error: {}", e);
                                            }
                                        } else {
                                            let mut any_routed = false;
                                            for doc_id in &doc_ids {
                                                match controller.on_block_changed(doc_id).await {
                                                    Ok(true) => { any_routed = true; }
                                                    Ok(false) => {} // doc_id not tracked
                                                    Err(e) => {
                                                        error!(
                                                            "[OrgMode] Block change error for {}: {}",
                                                            doc_id, e
                                                        );
                                                    }
                                                }
                                            }
                                            // If no doc_id resolved to a tracked file,
                                            // the routing was wrong — fall back to full re-render.
                                            if !any_routed {
                                                if let Err(e) = controller.re_render_all_tracked().await {
                                                    error!("[OrgMode] re_render_all_tracked fallback error: {}", e);
                                                }
                                            }
                                        }
                                        // Advance the `org` consumer watermark so test
                                        // settle-waits (`wait_for_consumers`) and
                                        // production observers can see when this
                                        // controller has finished with each event.
                                        // Marks even on per-doc errors so the
                                        // watermark is never permanently stuck —
                                        // genuine failures are surfaced via
                                        // `error!()` and the test's error tracker.
                                        if let Err(e) = event_bus_for_ctrl.mark_processed(&event_id, "org").await {
                                            tracing::warn!(
                                                "[OrgMode] mark_processed(org, {}) failed: {}",
                                                event_id, e
                                            );
                                        }
                                        idle_signal_for_task.mark_progress();
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to start file watcher: {}", e);
                            error!("[OrgMode] {}", msg);
                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_error(msg);
                            }
                        }
                    }
                });
            }

            // Always use SQL ops for write path consistency with QueryableCache reads.
            let wrapper =
                OperationWrapper::new(sql_ops.clone(), Some(sync_provider));
            Arc::new(wrapper) as Arc<dyn OperationProvider>
        }}));

        Ok(())
    }
}

/// Extract unique document IDs from an EventBus event.
///
/// For block.created/block.updated events, we look at the block's parent_id in the payload.
/// Document IDs are identified by the "doc:" URI scheme.
fn extract_doc_ids_from_event(event: &holon::sync::event_bus::Event) -> Vec<EntityUri> {
    use holon::sync::event_bus::EventKind;
    use std::collections::HashSet;

    let mut doc_ids = HashSet::new();

    match event.event_kind {
        EventKind::Created | EventKind::Updated | EventKind::Deleted | EventKind::FieldsChanged => {
            // Check _routing_doc_uri first (set by prepare_update for routing
            // without corrupting the block's actual parent_id)
            if let Some(doc_uri) = event
                .payload
                .get(holon::sync::event_bus::ROUTING_DOC_URI_KEY)
                .and_then(|v| v.as_str())
            {
                if let Ok(uri) = holon_api::EntityUri::parse(doc_uri) {
                    doc_ids.insert(uri);
                }
            }
            // Fall back to parent_id in data (for create/delete events)
            if doc_ids.is_empty() {
                if let Some(data) = event.payload.get("data") {
                    if let Some(parent_id) = data.get("parent_id").and_then(|v| v.as_str()) {
                        if let Ok(uri) = holon_api::EntityUri::parse(parent_id) {
                            doc_ids.insert(uri);
                        }
                    }
                }
            }
        }
    }

    doc_ids.into_iter().collect()
}

/// Extension trait for registering OrgMode services in a [`Injector`]
///
/// This trait provides a convenient method to register all OrgMode-related
/// services with a single call, taking just the root directory as a parameter.
///
/// # Example
///
/// ```rust,ignore
/// use holon_orgmode::di::OrgModeInjectorExt;
/// use std::path::PathBuf;
///
/// // In your DI setup closure:
/// services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
/// ```
pub trait OrgModeInjectorExt {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error>;
}

impl OrgModeInjectorExt for Injector {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error> {
        self.provide::<OrgModeConfig>(Provider::root(move |_| {
            Shared::new(OrgModeConfig::new(root_directory.clone()))
        }));
        OrgModeModule.configure(self)?;
        Ok(())
    }
}
