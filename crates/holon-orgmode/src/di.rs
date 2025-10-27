//! Dependency Injection module for OrgMode integration
//!
//! This module provides DI registration for OrgMode-specific services using ferrous-di.
//! OrgMode is now independent of Loro — it will use LoroBlockOperations if available in DI,
//! otherwise falls back to SqlOperationProvider for direct database writes.
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_orgmode::di::OrgModeServiceCollectionExt;
//! use std::path::PathBuf;
//!
//! services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
//! ```

use ferrous_di::{
    DiResult, Lifetime, Resolver, ServiceCollection, ServiceCollectionModuleExt, ServiceModule,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use holon_filesystem::directory::Directory;
use holon_filesystem::File;

use crate::file_watcher::OrgFileWatcher;
use crate::org_renderer::OrgRenderer;
use crate::org_sync_controller::OrgSyncController;
use crate::orgmode_event_adapter::OrgModeEventAdapter;
use crate::traits::{BlockReader, DocumentManager};
use crate::OrgModeSyncProvider;
use holon::core::datasource::{
    EntitySchemaProvider, OperationProvider, SyncTokenStore, SyncableProvider,
};
use holon::core::operation_wrapper::OperationWrapper;
use holon::core::queryable_cache::QueryableCache;
use holon::sync::event_bus::{EventBus, PublishErrorTracker};
use holon::sync::{
    Document, DocumentOperations, LoroBlockOperations, LoroDocumentStore, TursoEventBus,
};
use holon_api::block::Block;
use holon_api::EntityUri;

/// Signal that indicates the FileWatcher is ready to receive file change events.
///
/// Tests can wait on this signal to ensure the file watcher is established
/// before making external file modifications.
#[derive(Clone)]
pub struct FileWatcherReadySignal {
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl FileWatcherReadySignal {
    /// Create a new ready signal (sender/receiver pair)
    pub fn new() -> (FileWatcherReadySender, Self) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        (FileWatcherReadySender { sender: tx }, Self { receiver: rx })
    }

    /// Wait until the file watcher is ready.
    ///
    /// Returns immediately if already ready, otherwise blocks until signaled.
    /// This takes &self (not &mut self) so it works with Arc<FileWatcherReadySignal>.
    pub async fn wait_ready(&self) {
        // If already ready, return immediately
        if *self.receiver.borrow() {
            return;
        }
        // Clone the receiver for mutable access (watch receivers are designed to be cloned)
        let mut receiver = self.receiver.clone();
        // Wait for the signal to become true
        let _ = receiver.wait_for(|ready| *ready).await;
    }

    /// Check if the file watcher is ready without blocking.
    pub fn is_ready(&self) -> bool {
        *self.receiver.borrow()
    }
}

/// Sender half of the FileWatcher ready signal
pub struct FileWatcherReadySender {
    sender: tokio::sync::watch::Sender<bool>,
}

impl FileWatcherReadySender {
    /// Signal that the file watcher is ready
    pub fn signal_ready(self) {
        let _ = self.sender.send(true);
    }
}

/// Scan a directory recursively for .org files.
///
/// Returns a list of paths to all .org files found.
fn scan_org_files(dir: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
    let mut org_files = Vec::new();

    if !dir.exists() {
        return Ok(org_files);
    }

    fn walk_dir(dir: &std::path::Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden directories
                if !path
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
                {
                    walk_dir(&path, files)?;
                }
            } else if path.extension().map(|e| e == "org").unwrap_or(false) {
                files.push(path);
            }
        }
        Ok(())
    }

    walk_dir(dir, &mut org_files)?;
    Ok(org_files)
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

    async fn iter_documents_with_blocks(&self) -> Vec<(EntityUri, Vec<Block>)> {
        use holon::core::datasource::DataSource;
        use std::collections::HashMap;

        let all_blocks = match self.cache.get_all().await {
            Ok(blocks) => blocks,
            Err(e) => {
                tracing::warn!("[CacheBlockReader] Failed to load blocks: {}", e);
                return Vec::new();
            }
        };

        // Build parent_id → children index for fast lookup
        // Key is EntityUri (cloned from block.parent_id) to stay typed
        let mut children_of: HashMap<EntityUri, Vec<&Block>> = HashMap::new();
        for block in &all_blocks {
            children_of
                .entry(block.parent_id.clone())
                .or_default()
                .push(block);
        }

        // Find all document URIs (top-level parent_ids with doc: scheme)
        let doc_uris: Vec<EntityUri> = children_of
            .keys()
            .filter(|pid| pid.is_doc())
            .cloned()
            .collect();

        // For each document, collect all descendants via BFS
        let mut result = Vec::new();
        for doc_uri in doc_uris {
            let mut blocks = Vec::new();
            let mut queue: Vec<EntityUri> = vec![doc_uri.clone()];
            while let Some(pid) = queue.pop() {
                if let Some(children) = children_of.get(&pid) {
                    for block in children {
                        queue.push(block.id.clone());
                        blocks.push((*block).clone());
                    }
                }
            }
            if !blocks.is_empty() {
                result.push((doc_uri, blocks));
            }
        }

        result
    }

    async fn find_foreign_blocks(
        &self,
        block_ids: &[EntityUri],
        expected_doc_uri: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }

        use holon::core::datasource::DataSource;
        use std::collections::HashSet;

        let id_set: HashSet<&EntityUri> = block_ids.iter().collect();

        let all_blocks = self
            .cache
            .get_all()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(all_blocks
            .iter()
            .filter(|b| id_set.contains(&b.id) && b.parent_id != *expected_doc_uri)
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect())
    }
}

/// DocumentManager backed by CDC-driven LiveData.
///
/// All reads (`find_by_parent_and_name`, `get_by_id`) are in-memory lookups
/// against a `LiveData<Document>` that stays current via a Turso materialized
/// view CDC stream. Writes go through `DocumentOperations` (SQL); the matview
/// CDC automatically propagates them into the LiveData.
///
/// This eliminates the ~1300 redundant `SELECT * FROM document WHERE …` SQL
/// queries that occurred during startup when every CDC block event triggered
/// document path resolution via SQL.
pub struct LiveDocumentManager {
    live: Arc<holon::sync::LiveData<Document>>,
    doc_ops: Arc<DocumentOperations>,
}

impl LiveDocumentManager {
    /// Create a LiveDocumentManager backed by a materialized view over `document`.
    pub async fn new(
        doc_ops: Arc<DocumentOperations>,
        backend: Arc<tokio::sync::RwLock<holon::storage::turso::TursoBackend>>,
    ) -> anyhow::Result<Self> {
        let backend_guard = backend.read().await;
        let db_handle = backend_guard.handle();
        drop(backend_guard);

        let matview_mgr =
            holon::sync::MatviewManager::new(db_handle, Arc::new(tokio::sync::Mutex::new(())));

        let result = matview_mgr.watch("SELECT * FROM document").await?;

        let live = holon::sync::LiveData::new(
            result.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("document row missing 'id'"))
            },
            |row| DocumentOperations::row_to_document(row).map_err(|e| anyhow::anyhow!("{}", e)),
        );
        live.subscribe(result.stream);

        tracing::info!(
            "[LiveDocumentManager] Watching {} documents via matview",
            live.read().len()
        );

        Ok(Self { live, doc_ops })
    }
}

#[async_trait::async_trait]
impl DocumentManager for LiveDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        name: &str,
    ) -> anyhow::Result<Option<Document>> {
        let docs = self.live.read();
        Ok(docs
            .values()
            .find(|d| d.parent_id == *parent_id && d.name == name)
            .cloned())
    }

    async fn create(&self, doc: Document) -> anyhow::Result<Document> {
        let created = self
            .doc_ops
            .create(doc)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        // Optimistic cache update: insert into LiveData immediately so that
        // subsequent find_by_parent_and_name calls see the new document without
        // waiting for the matview CDC roundtrip.
        self.live
            .insert(created.id.as_str().to_string(), created.clone());
        Ok(created)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Document>> {
        let docs = self.live.read();
        Ok(docs.get(id.as_str()).cloned())
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
/// Entity schema provider for File and Directory (filesystem entities).
struct FilesystemEntitySchemaProvider;

impl EntitySchemaProvider for FilesystemEntitySchemaProvider {
    fn entity_schemas(&self) -> Vec<holon_api::EntitySchema> {
        vec![File::entity_schema(), Directory::entity_schema()]
    }
}

/// OrgMode will detect if LoroBlockOperations is available in DI and use it;
/// otherwise it falls back to SqlOperationProvider.
pub struct OrgModeModule;

impl ServiceModule for OrgModeModule {
    fn register_services(self, services: &mut ServiceCollection) -> DiResult<()> {
        use tracing::{error, info};

        info!("[OrgModeModule] register_services called");

        // Register filesystem entity schemas for GQL graph
        services.add_trait_factory::<dyn EntitySchemaProvider, _>(Lifetime::Singleton, |_| {
            Arc::new(FilesystemEntitySchemaProvider) as Arc<dyn EntitySchemaProvider>
        });

        // Create and register FileWatcherReadySignal
        // Tests can wait on this to ensure file watcher is ready before external mutations
        let (ready_sender, ready_signal) = FileWatcherReadySignal::new();
        services.add_singleton(ready_signal);
        // Store sender in Arc<Mutex> so we can move it into the spawned task later
        let ready_sender = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_sender)));
        let ready_sender_for_factory = ready_sender.clone();

        // Register OrgModeSyncProvider as a factory
        services.add_singleton_factory::<OrgModeSyncProvider, _>(|resolver| {
            let config = resolver.get_required::<OrgModeConfig>();
            let token_store = resolver
                .get_trait::<dyn SyncTokenStore>()
                .expect("[OrgModeModule] SyncTokenStore not found in DI");
            OrgModeSyncProvider::new(config.root_directory.clone(), token_store)
        });

        // Register SyncableProvider trait implementation
        services.add_trait_factory::<dyn SyncableProvider, _>(Lifetime::Singleton, |resolver| {
            let sync_provider = resolver.get_required::<OrgModeSyncProvider>();
            sync_provider.clone() as Arc<dyn SyncableProvider>
        });

        // Register OrgMode-specific QueryableCaches (Block cache is registered by FrontendConfig)
        services.add_singleton_factory::<QueryableCache<Directory>, _>(|r| {
            holon::di::create_queryable_cache(r)
        });
        services.add_singleton_factory::<QueryableCache<Document>, _>(|r| {
            holon::di::create_queryable_cache(r)
        });
        services.add_singleton_factory::<QueryableCache<File>, _>(|r| {
            holon::di::create_queryable_cache(r)
        });

        // Register DocumentOperations
        services.add_singleton_factory::<DocumentOperations, _>(|resolver| {
            let db_handle_provider =
                resolver.get_required_trait::<dyn holon::di::DbHandleProvider>();
            let cache = resolver.get_required::<QueryableCache<Document>>();

            let ops = DocumentOperations::new(db_handle_provider.handle(), cache);

            // Initialize schema synchronously
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    ops.init_schema()
                        .await
                        .expect("Failed to initialize documents schema");
                })
            });

            ops
        });

        // Register DocumentOperations as OperationProvider for "document" entity
        services.add_trait_factory::<dyn OperationProvider, _>(Lifetime::Singleton, |resolver| {
            let doc_ops = resolver.get_required::<DocumentOperations>();
            doc_ops as Arc<dyn OperationProvider>
        });

        // TursoEventBus is registered by FrontendConfig shared infrastructure

        // Register OrgRenderer
        services.add_singleton_factory::<Arc<OrgRenderer>, _>(|_resolver| Arc::new(OrgRenderer));

        // Set up event bus wiring and background tasks.
        // This factory resolves LoroBlockOperations if available (Loro enabled),
        // otherwise creates a SqlOperationProvider for direct SQL block operations.
        services.add_trait_factory::<dyn OperationProvider, _>(
            Lifetime::Singleton,
            move |resolver| {
                // ============================================================
                // PHASE 1: Resolve ALL services that run DDL
                // This ensures all schema initialization completes BEFORE
                // any background tasks start using the database.
                // ============================================================
                info!("[OrgMode] Phase 1: Resolving services (DDL)");

                let _dir_cache = resolver.get_required::<QueryableCache<Directory>>();
                let _file_cache = resolver.get_required::<QueryableCache<File>>();
                let _block_cache = resolver.get_required::<QueryableCache<Block>>();
                let sync_provider = resolver.get_required::<OrgModeSyncProvider>();

                // IMPORTANT: Resolve TursoEventBus HERE, not after spawns!
                // TursoEventBus::init_schema() runs DDL that must complete
                // before any background tasks use the database.
                let event_bus = resolver.get_required::<TursoEventBus>();
                let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();

                // Resolve remaining services that might run DDL
                let config = resolver.get_required::<OrgModeConfig>();
                let doc_ops = resolver.get_required::<DocumentOperations>();

                // Try to resolve Loro services (available if LoroModule was registered)
                let loro_ops: Option<Arc<LoroBlockOperations>> =
                    resolver.get::<LoroBlockOperations>().ok();

                let loro_available = loro_ops.is_some();
                info!(
                    "[OrgMode] Phase 1 complete: All DDL finished (loro={})",
                    loro_available
                );

                // Resolve DbHandle unconditionally — Turso is always available
                let db_handle_provider =
                    resolver.get_required_trait::<dyn holon::di::DbHandleProvider>();
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

                // LoroBlockOperations → EventBus is wired by LoroModule (LoroEventAdapterHandle)

                // OrgModeSyncProvider → EventBus (directories and files only)
                {
                    let sync_provider_clone = sync_provider.clone();
                    let event_bus_clone = event_bus_arc.clone();
                    let error_tracker = resolver.get::<PublishErrorTracker>()
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
                    let doc_ops_clone = doc_ops.clone();
                    let event_bus_for_ctrl = event_bus_arc.clone();
                    let ready_sender_clone = ready_sender_for_factory.clone();

                    let loro_ops_clone = loro_ops.clone();
                    let block_cache = resolver.get_required::<QueryableCache<Block>>();
                    let backend_provider =
                        resolver.get_required_trait::<dyn holon::di::TursoBackendProvider>();
                    let backend_for_live_docs = backend_provider.backend();

                    tokio::spawn(async move {
                        let doc_manager: Arc<dyn DocumentManager> = Arc::new(
                            LiveDocumentManager::new(doc_ops_clone, backend_for_live_docs)
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

                        controller.initialize().await;

                        // Subscribe to EventBus for block events
                        let block_filter = holon::sync::event_bus::EventFilter::new()
                            .with_aggregate_type(holon::sync::event_bus::AggregateType::Block)
                            .with_status(holon::sync::event_bus::EventStatus::Confirmed);
                        let mut event_rx = match event_bus_for_ctrl.subscribe(block_filter).await {
                            Ok(rx) => rx,
                            Err(e) => {
                                error!("[OrgMode] Failed to subscribe to EventBus: {}", e);
                                if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                    sender.signal_ready();
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
                                if let Ok(existing_files) =
                                    scan_org_files(&config_clone.root_directory)
                                {
                                    for file_path in existing_files {
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
                                }

                                if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                    sender.signal_ready();
                                }

                                // Main loop: handle file changes and EventBus block events
                                loop {
                                    tokio::select! {
                                        Some(file_path) = file_rx.recv() => {
                                            if let Err(e) = controller.on_file_changed(&file_path).await {
                                                error!(
                                                    "[OrgMode] File change error {}: {}",
                                                    file_path.display(), e
                                                );
                                            }
                                        }
                                        Some(event) = tokio_stream::StreamExt::next(&mut event_rx) => {
                                            let doc_ids = extract_doc_ids_from_event(&event);
                                            if doc_ids.is_empty() {
                                                // No doc_id extractable — re-render all tracked files
                                                if let Err(e) = controller.re_render_all_tracked().await {
                                                    error!(
                                                        "[OrgMode] Re-render all error: {}",
                                                        e
                                                    );
                                                }
                                            } else {
                                                for doc_id in doc_ids {
                                                    if let Err(e) = controller.on_block_changed(&doc_id).await {
                                                        error!(
                                                            "[OrgMode] Block change error for {}: {}",
                                                            doc_id, e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("[OrgMode] Failed to start file watcher: {}", e);
                                if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                    sender.signal_ready();
                                }
                            }
                        }
                    });
                }

                // Always use SQL ops for write path consistency with QueryableCache reads.
                let wrapper =
                    OperationWrapper::new(sql_ops.clone(), Some(sync_provider));
                Arc::new(wrapper) as Arc<dyn OperationProvider>
            },
        );

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
                    if uri.is_doc() {
                        doc_ids.insert(uri);
                    }
                }
            }
            // Fall back to parent_id in data (for create/delete events)
            if doc_ids.is_empty() {
                if let Some(data) = event.payload.get("data") {
                    if let Some(parent_id) = data.get("parent_id").and_then(|v| v.as_str()) {
                        if let Ok(uri) = holon_api::EntityUri::parse(parent_id) {
                            if uri.is_doc() {
                                doc_ids.insert(uri);
                            }
                        }
                    }
                }
            }
        }
    }

    doc_ids.into_iter().collect()
}

/// Extension trait for registering OrgMode services in a [`ServiceCollection`]
///
/// This trait provides a convenient method to register all OrgMode-related
/// services with a single call, taking just the root directory as a parameter.
///
/// # Example
///
/// ```rust,ignore
/// use holon_orgmode::di::OrgModeServiceCollectionExt;
/// use std::path::PathBuf;
///
/// // In your DI setup closure:
/// services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
/// ```
pub trait OrgModeServiceCollectionExt {
    /// Register OrgMode services with the given root directory
    ///
    /// This registers:
    /// - `OrgModeConfig` with the provided root directory
    /// - `OrgModeModule` which sets up all OrgMode-related services
    ///
    /// # Errors
    ///
    /// Returns an error if module registration fails.
    fn add_orgmode(&mut self, root_directory: PathBuf) -> DiResult<()>;
}

impl OrgModeServiceCollectionExt for ServiceCollection {
    fn add_orgmode(&mut self, root_directory: PathBuf) -> DiResult<()> {
        self.add_singleton(OrgModeConfig::new(root_directory));
        self.add_module_mut(OrgModeModule)?;
        Ok(())
    }
}
