//! Test environment for integration tests
//!
//! Provides a high-level wrapper around BackendEngine for testing.
//! Uses FrontendSession from holon-frontend to ensure identical initialization
//! path with production frontends (Flutter, TUI, etc.).
//!
//! ## Pre-Startup Testing
//!
//! TestEnvironment supports two phases:
//! 1. **Pre-startup** (`session: None`): Can write org files to temp_dir before the app starts
//! 2. **Running** (`session: Some`): Full application functionality
//!
//! This enables testing scenarios where files exist before the application starts,
//! reproducing the Flutter startup bug where DDL operations race with sync of existing files.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::{assign_reference_sequences, wait_for_file_condition};
use holon_api::reactive::CdcAccumulator;

use holon::api::loro_backend::LoroBackend;
use holon::api::{BackendEngine, RowChangeStream};
use holon::di::{StorageSelector, build_no_turso_container};
use holon::sync::LoroDocumentStore;
use holon::sync::event_bus::PublishErrorTracker;
use holon::sync::loro_block_query_source::{
    register_loro_block_query_source, register_loro_operation_engine,
};
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::EntityUri;
use holon_api::QueryContext;
use holon_api::block::Block;
use holon_api::{ContentType, QueryLanguage, Region, RenderExpr, SourceLanguage, Value};
use holon_app::register_block_query_frontend;
use holon_filesystem::FileSystem;
use holon_frontend::reactive::{BuilderServices, BuilderServicesSlot, ReactiveEngine};
use holon_frontend::{FrontendSession, HolonConfig, SessionConfig};

/// Types of corruption for stale .loro files (for testing recovery)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LoroCorruptionType {
    /// Empty file (0 bytes)
    Empty,
    /// File with partial/truncated Loro header
    Truncated,
    /// File with invalid magic bytes
    InvalidHeader,
}

/// Resolve the DI-registered `DebugServices` and pre-populate its
/// optional fields (`loro_doc_store`) from other DI services. Mirrors
/// what `holon_mcp::di::DebugServicesPopulatorModule` does for
/// module-using consumers — the test path runs it inline because the
/// `extra_resolve` callback is the natural post-`on_start` hook.
fn populate_debug_services(injector: &fluxdi::Injector) -> Arc<holon_mcp::server::DebugServices> {
    let debug = injector.resolve::<holon_mcp::server::DebugServices>();
    if let Ok(ops) = injector.try_resolve::<holon::sync::LoroBlockOperations>() {
        debug.loro_doc_store.set(ops.shared_doc_store()).ok();
    }
    debug
}

/// Build a pre-filled `OnceCell` for a struct literal. Infallible: the cell is
/// fresh, so `set` cannot fail.
fn filled_once_cell<T>(value: T) -> OnceCell<T> {
    let cell = OnceCell::new();
    if cell.set(value).is_err() {
        unreachable!("fresh OnceCell::set cannot fail");
    }
    cell
}

/// Build a `OnceCell` from an `Option`: filled when `Some`, empty when `None`.
fn once_cell_from_option<T>(value: Option<T>) -> OnceCell<T> {
    match value {
        Some(value) => filled_once_cell(value),
        None => OnceCell::new(),
    }
}

/// Test environment with optional running application.
///
/// Supports two phases:
/// - Pre-startup (session: None): Can write org files, loro files to temp_dir
/// - Running (session: Some): Full application functionality
pub struct TestEnvironment {
    /// Temp directory for Org files
    pub temp_dir: TempDir,

    /// Runtime for async operations
    pub runtime: Arc<tokio::runtime::Runtime>,

    /// The running application (empty before start_app()).
    ///
    /// `OnceCell` so `start_app` can latch it via `&self` (the prerequisite for
    /// an `&self` `apply_start_app`). `OnceCell::get()` hands out a borrow-guard-free
    /// `&FrontendSession`, so the `session()`/`test_ctx()` accessors keep returning
    /// `&T` and holding it across `.await` is sound. The rare config-change restart
    /// (`stop_app`) resets via `OnceCell::take()` — available because `stop_app`
    /// keeps `&mut self`.
    session: OnceCell<Arc<FrontendSession>>,

    /// DI injector clone, captured during startup. Lets read-only inspection
    /// (e.g. `snapshot_org_render_pairs`) resolve the production
    /// `QueryableCache<Block>` and render through the *same* `CacheBlockReader`
    /// the `FileSyncController` uses — no bespoke query that could drift from
    /// production ordering. `None` before `start_app()`.
    injector: OnceCell<fluxdi::Injector>,

    /// Loro doc store, resolved from DI (empty when Loro is disabled)
    loro_doc_store: OnceCell<Arc<RwLock<LoroDocumentStore>>>,

    /// MCP DebugServices, resolved from DI and pre-populated via
    /// [`populate_debug_services`]. Threaded into the embedded MCP
    /// server (`try_start_embedded_mcp`) so inspection tools work in
    /// PBTs.
    debug_services: OnceCell<Arc<holon_mcp::server::DebugServices>>,

    /// Loro sync controller handle, resolved from DI (None when Loro is disabled).
    /// Used by `wait_for_loro_quiescence` to poll until the controller has
    /// caught up with the current Loro state.
    loro_sync_handle: OnceCell<Arc<holon::sync::LoroSyncControllerHandle>>,

    /// Reactive engine, resolved from DI (same instance as GPUI uses).
    /// Provides BuilderServices, keybinding registry, operation dispatch.
    pub reactive_engine: OnceCell<Arc<holon_frontend::reactive::ReactiveEngine>>,

    /// Idle signal for the FileSyncController loop. When present, lets
    /// `wait_for_org_files_stable` skip filesystem polling on the hot path.
    org_sync_idle: OnceCell<Arc<holon_orgmode::OrgSyncIdleSignal>>,

    /// The E2ETestContext for operations (wraps BackendEngine) - only valid after start_app()
    ctx: OnceCell<E2ETestContext>,

    /// Created documents (doc_uri -> file path).
    ///
    /// Interior-mutable so the org-file write path (`write_org_file`,
    /// `create_document`) and the post-ingest re-key in `apply_write_org_file`
    /// can be `&self` — the prerequisite for decomposing those transitions onto
    /// `&self` caps. Same soundness as [`Self::active_watches`]: no borrow ever
    /// crosses an `.await` (each call takes/drops the guard around the await),
    /// and `E2ESut` is never `Send`-bound.
    pub documents: RefCell<HashMap<EntityUri, PathBuf>>,

    /// Active CDC watches (query_id -> stream).
    ///
    /// Interior-mutable so the watch-write path (`setup_watch`) can be `&self` —
    /// the prerequisite for decomposing the `SetupWatch` transition onto the
    /// fine-grained `SutWatchRegister` cap (`&self`, as all `capmap_adapter` caps
    /// must be). No borrow is ever held across an `.await`, and `E2ESut` is never
    /// `Send`-bound (all its async cap impls are `?Send`, driven via `block_on`),
    /// so `RefCell` is sound here.
    pub active_watches: RefCell<HashMap<String, RowChangeStream>>,

    /// Watch query metadata for fallback re-query (query_id -> (source, language)).
    /// Interior-mutable for the same reason as [`Self::active_watches`].
    pub watch_queries: RefCell<HashMap<String, (String, QueryLanguage)>>,

    /// UI model built from CDC events (query_id -> accumulator).
    /// Interior-mutable for the same reason as [`Self::active_watches`].
    pub ui_model: RefCell<HashMap<String, CdcAccumulator<holon_api::StorageEntity>>>,

    /// Current view filter. Interior-mutable so `switch_view` can be `&self`
    /// (same soundness rationale as [`Self::active_watches`]).
    pub current_view: RefCell<String>,

    /// Region CDC streams from AppFrame (region_id -> stream). Interior-mutable
    /// so `setup_region_watch` (reached from the now-`&self` `apply_start_app`)
    /// can insert via `&self`; same `?Send`/no-borrow-across-`.await` soundness
    /// as [`Self::active_watches`].
    pub region_streams: RefCell<HashMap<String, RowChangeStream>>,

    /// Region data built from CDC events (region_id -> accumulator). Interior-mutable
    /// for the same reason as [`Self::region_streams`].
    pub region_data: RefCell<HashMap<String, CdcAccumulator<holon_api::StorageEntity>>>,

    /// All-blocks CDC watch for invariant #1 (uses production CdcAccumulator).
    ///
    /// Interior-mutable so the block-convergence settle (`wait_for_blocks_synced`,
    /// reached from `simulate_restart`) can be `&self`. The await-driven drain
    /// loops `take()` the accumulator+stream into locals (dropping the guard)
    /// before any `.await`, then restore them — no `RefCell` borrow ever crosses
    /// a suspension point. Same `?Send`/single-threaded soundness as
    /// [`Self::active_watches`].
    pub all_blocks: RefCell<Option<CdcAccumulator<holon_api::StorageEntity>>>,

    /// All-blocks CDC stream. Interior-mutable for the same reason as
    /// [`Self::all_blocks`].
    all_blocks_stream: RefCell<Option<RowChangeStream>>,

    /// Number of production blocks that don't appear in `ref_state`
    /// (sentinel:no_parent, default sidebars, etc.). Captured by
    /// `prime_seed_count` once the initial app state has fully synced
    /// into `all_blocks`. Used by `wait_for_blocks_synced` to detect
    /// pending deletes (subset alone can't — acc still holds the
    /// deleted block until the CDC delete event arrives).
    /// Interior-mutable (`Cell`) for the same reason as [`Self::all_blocks`].
    seed_count: Cell<Option<usize>>,

    /// Whether to enable Todoist fake mode (adds concurrent DDL during startup).
    /// `Cell` so `set_enable_fake_mcp` can write via `&self`.
    enable_fake_mcp: Cell<bool>,

    /// Whether to enable Loro CRDT layer (default: true for backward compat).
    /// `Cell` so `set_enable_loro` can write via `&self`.
    enable_loro: Cell<bool>,

    /// Which storage substrate `start_app` assembles (default: `Turso`).
    storage: StorageSelector,

    /// The in-memory Loro backend for a `LoroMemory` session — the storage
    /// adapter the SUT seeds / mutates directly (the no-Turso wiring has no
    /// engine dispatch). Created and registered into the DI container at
    /// `start_app`; `None` for a Turso session or before startup.
    loro_backend: OnceCell<Arc<LoroBackend>>,

    /// Idle signal + keepalive for the no-Turso `FileSyncController` loop started
    /// by resolving `FileSyncStarted`. Holding this strong `Arc` keeps the loop
    /// alive (the loop holds a `Weak` and exits when this drops). `None` for a
    /// Turso session or before startup.
    loro_org_idle: OnceCell<std::sync::Arc<holon_orgmode::di::OrgSyncIdleSignal>>,

    /// Shared in-memory org filesystem (ADR 0011 P3). All harness org-file
    /// I/O goes through it, and `start_app` overrides the DI `FileSystem` /
    /// `FileChangeSource` bindings so the production org sync path reads and
    /// writes the SAME instance — a write fires the change event synchronously
    /// on completion, making org sync deterministic (no fsevents debounce,
    /// no partial-write window). Survives app restarts within one env.
    pub org_fs: Arc<holon_filesystem::InMemoryFileSystem>,

    /// Canonicalized temp-dir path — the org root as the controller sees it
    /// (`OrgModeConfig::new` canonicalizes; macOS `/var` → `/private/var`).
    /// All in-memory org paths must be built from this so `CanonicalPath`
    /// (which falls back to the raw path for files not on the real disk)
    /// yields keys that strip_prefix against the controller root.
    org_root: PathBuf,
}

impl std::fmt::Debug for TestEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestEnvironment")
            .field("documents", &self.documents)
            .field("temp_dir", &self.temp_dir.path())
            .field("is_running", &self.session.get().is_some())
            .finish_non_exhaustive()
    }
}

/// Builder for TestEnvironment that allows pre-populating org files before engine initialization.
///
/// This is critical for reproducing the Flutter startup bug where:
/// 1. Org files already exist when the app starts
/// 2. OrgModeSyncProvider scans and emits ALL existing files/blocks
/// 3. preload_startup_views runs DDL concurrently with event publishing
/// 4. Events are dropped due to "Database schema changed" errors
///
/// # Example
/// ```rust,ignore
/// let env = TestEnvironmentBuilder::new()
///     .with_org_file("test.org", "* Headline 1\n:PROPERTIES:\n:ID: block-1\n:END:\n")
///     .with_org_file("test2.org", "* Headline 2\n:PROPERTIES:\n:ID: block-2\n:END:\n")
///     .wait_for_file_watcher(false)  // Don't wait - capture the race
///     .build(runtime)
///     .await?;
///
/// // Check for startup errors
/// assert!(!env.has_startup_errors(), "Startup should not have errors");
/// ```
pub struct TestEnvironmentBuilder {
    /// Pre-populated org files (filename -> content)
    org_files: Vec<(String, String)>,
    /// Whether to wait for file watcher to be ready before returning
    wait_for_file_watcher: bool,
    /// Additional delay after file watcher ready (ms)
    settle_delay_ms: u64,
    /// Enable a fake external MCP provider (for testing DDL race conditions)
    enable_fake_mcp: bool,
    /// Enable Loro CRDT layer (default: true)
    enable_loro: bool,
}

impl TestEnvironmentBuilder {
    /// Create a new TestEnvironmentBuilder
    pub fn new() -> Self {
        Self {
            org_files: Vec::new(),
            wait_for_file_watcher: true,
            settle_delay_ms: 100,
            enable_fake_mcp: false,
            enable_loro: true,
        }
    }

    /// Add an org file to be created BEFORE engine initialization
    ///
    /// The file will exist when OrgModeSyncProvider starts scanning,
    /// which triggers the sync/DDL race condition.
    pub fn with_org_file(
        mut self,
        filename: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        self.org_files.push((filename.into(), content.into()));
        self
    }

    /// Set whether to wait for file watcher to be ready before returning
    ///
    /// Set to `false` to capture the race condition where events are published
    /// while preload_views is still running DDL.
    pub fn wait_for_file_watcher(mut self, wait: bool) -> Self {
        self.wait_for_file_watcher = wait;
        self
    }

    /// Set the delay after file watcher is ready (in milliseconds)
    ///
    /// Only applies if `wait_for_file_watcher` is true.
    pub fn settle_delay_ms(mut self, ms: u64) -> Self {
        self.settle_delay_ms = ms;
        self
    }

    /// Enable a fake external MCP provider via an in-memory duplex transport.
    ///
    /// Drives the real MCP client pipeline (McpSyncEngine → QueryableCache →
    /// Turso), creating its cache table and running an initial sync concurrently
    /// with startup. Replaces the old Todoist fake as the concurrent-DDL race
    /// stressor — see `fake_mcp_module`.
    pub fn with_fake_mcp(mut self) -> Self {
        self.enable_fake_mcp = true;
        self
    }

    /// Disable Loro CRDT layer. Matches the Flutter production path when
    /// LORO_ENABLED is not set (the default).
    pub fn without_loro(mut self) -> Self {
        self.enable_loro = false;
        self
    }

    /// Build the TestEnvironment, creating any pre-populated org files first
    ///
    /// Uses FrontendSession to ensure identical initialization path with production frontends.
    /// This simulates the Flutter scenario where files exist before the app starts.
    pub async fn build(self, runtime: Arc<tokio::runtime::Runtime>) -> Result<TestEnvironment> {
        let temp_dir =
            TempDir::new().map_err(|e| anyhow::anyhow!("Failed to create temp dir: {}", e))?;
        let org_root = std::fs::canonicalize(temp_dir.path())
            .map_err(|e| anyhow::anyhow!("Failed to canonicalize temp dir: {}", e))?;
        let org_fs = Arc::new(holon_filesystem::InMemoryFileSystem::new());
        org_fs.mkdir_all(&org_root);

        // Write pre-populated org files BEFORE engine initialization
        // This is the key to reproducing the Flutter bug
        let mut documents = HashMap::new();
        for (filename, content) in &self.org_files {
            let file_path = org_root.join(filename);
            if let Some(parent) = file_path.parent() {
                org_fs.mkdir_all(parent);
            }
            holon_filesystem::FileSystem::write(org_fs.as_ref(), &file_path, content.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write pre-populated org file: {}", e))?;

            let doc_uri = EntityUri::file(filename);
            documents.insert(doc_uri, file_path);
        }

        let enable_loro = self.enable_loro;

        let settle_delay_ms = self.settle_delay_ms;

        let holon_config = HolonConfig {
            db_path: Some(temp_dir.path().join("test.db")),
            orgmode: holon_frontend::config::OrgmodeConfig {
                root_directory: Some(temp_dir.path().to_path_buf()),
            },
            loro: holon_frontend::config::LoroPreferences {
                enabled: if enable_loro { Some(true) } else { None },
                ..Default::default()
            },
            ..Default::default()
        };
        let config_dir = temp_dir.path().to_path_buf();
        let mut session_config = SessionConfig::new(holon_api::UiInfo::permissive());
        if !self.wait_for_file_watcher {
            session_config = session_config.without_wait();
        }
        let enable_fake_mcp = self.enable_fake_mcp;
        let org_fs_for_di = org_fs.clone();

        let (
            session,
            backend_engine,
            (doc_store, reactive_engine, sync_handle, idle_signal, debug_services, injector),
        ) = holon_app::new_from_config_with_di(
            holon_config,
            session_config,
            config_dir,
            std::collections::HashSet::new(),
            move |injector| {
                use holon_frontend::reactive::{BuilderServicesSlot, RenderInterpreterInjectorExt};
                override_org_fs_bindings(injector, &org_fs_for_di);
                let slot = injector.resolve::<BuilderServicesSlot>();
                injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                    slot.0.clone(),
                ));
                holon_mcp::di::register_debug_services(injector);
                if enable_fake_mcp {
                    crate::fake_mcp_module::register_fake_mcp(injector);
                }
                Ok(())
            },
            move |injector| {
                use holon_frontend::reactive::{
                    BuilderServices, BuilderServicesSlot, ReactiveEngine,
                };
                let engine = injector.resolve::<ReactiveEngine>();
                let slot = injector.resolve::<BuilderServicesSlot>();
                let services: Arc<dyn BuilderServices> = engine.clone();
                slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent

                let doc_store = if enable_loro {
                    injector
                        .try_resolve::<LoroDocumentStore>()
                        .ok() // ALLOW(ok): optional DI service
                        .map(|store| Arc::new(RwLock::new((*store).clone())))
                } else {
                    None
                };
                let sync_handle = if enable_loro {
                    injector
                        .try_resolve::<holon::sync::LoroSyncControllerHandle>()
                        .ok()
                } else {
                    None
                };
                let idle_signal = injector
                    .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                    .ok();
                let debug_services = populate_debug_services(injector);
                (
                    doc_store,
                    engine,
                    sync_handle,
                    idle_signal,
                    debug_services,
                    injector.clone(),
                )
            },
        )
        .await?;

        // Tests need deterministic state — wait for CDC event propagation
        if settle_delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(settle_delay_ms)).await;
        }

        let ctx = E2ETestContext::from_engine(backend_engine);

        let _startup_errors = session.error_tracker().errors();

        Ok(TestEnvironment {
            org_fs,
            org_root,
            temp_dir,
            runtime,
            session: filled_once_cell(session),
            injector: filled_once_cell(injector),
            loro_doc_store: once_cell_from_option(doc_store),
            debug_services: filled_once_cell(debug_services),
            loro_sync_handle: once_cell_from_option(sync_handle),
            reactive_engine: filled_once_cell(reactive_engine),
            org_sync_idle: once_cell_from_option(idle_signal),
            ctx: filled_once_cell(ctx),
            documents: RefCell::new(documents),
            active_watches: RefCell::new(HashMap::new()),
            watch_queries: RefCell::new(HashMap::new()),
            ui_model: RefCell::new(HashMap::new()),
            current_view: RefCell::new("all".to_string()),
            region_streams: RefCell::new(HashMap::new()),
            region_data: RefCell::new(HashMap::new()),
            all_blocks: RefCell::new(None),
            all_blocks_stream: RefCell::new(None),
            seed_count: Cell::new(None),
            enable_fake_mcp: Cell::new(self.enable_fake_mcp),
            enable_loro: Cell::new(enable_loro),
            storage: StorageSelector::Turso,
            loro_backend: OnceCell::new(),
            loro_org_idle: OnceCell::new(),
        })
    }
}

impl Default for TestEnvironmentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Replace the production org-file adapters with the env-shared in-memory
/// filesystem (ADR 0011 P3). Runs in `extra_setup`, after `add_frontend`
/// registered the real-disk defaults and before anything resolves them.
/// A write through this fs fires its change event synchronously on
/// completion — org sync becomes deterministic (no fsevents debounce,
/// no partial-write window, no 9s recursive-watch arming).
pub(crate) fn override_org_fs_bindings(
    injector: &fluxdi::Injector,
    org_fs: &Arc<holon_filesystem::InMemoryFileSystem>,
) {
    let fs = org_fs.clone();
    injector.override_provider::<dyn holon_filesystem::FileSystem>(fluxdi::Provider::root(
        move |_| fs.clone() as Arc<dyn holon_filesystem::FileSystem>,
    ));
    let cs = org_fs.clone();
    injector.override_provider::<dyn holon_filesystem::FileChangeSource>(fluxdi::Provider::root(
        move |_| cs.clone() as Arc<dyn holon_filesystem::FileChangeSource>,
    ));
}

impl TestEnvironment {
    /// Create a new test environment (app not started yet).
    ///
    /// Use this for pre-startup testing scenarios. Call `start_app()` to start the application.
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        let temp_dir =
            TempDir::new().map_err(|e| anyhow::anyhow!("Failed to create temp dir: {}", e))?;
        let org_root = std::fs::canonicalize(temp_dir.path())
            .map_err(|e| anyhow::anyhow!("Failed to canonicalize temp dir: {}", e))?;
        let org_fs = Arc::new(holon_filesystem::InMemoryFileSystem::new());
        org_fs.mkdir_all(&org_root);

        Ok(Self {
            org_fs,
            org_root,
            temp_dir,
            runtime,
            session: OnceCell::new(),
            injector: OnceCell::new(),
            loro_doc_store: OnceCell::new(),
            debug_services: OnceCell::new(),
            loro_sync_handle: OnceCell::new(),
            reactive_engine: OnceCell::new(),
            org_sync_idle: OnceCell::new(),
            ctx: OnceCell::new(),
            documents: RefCell::new(HashMap::new()),
            active_watches: RefCell::new(HashMap::new()),
            watch_queries: RefCell::new(HashMap::new()),
            ui_model: RefCell::new(HashMap::new()),
            current_view: RefCell::new("all".to_string()),
            region_streams: RefCell::new(HashMap::new()),
            region_data: RefCell::new(HashMap::new()),
            all_blocks: RefCell::new(None),
            all_blocks_stream: RefCell::new(None),
            seed_count: Cell::new(None),
            enable_fake_mcp: Cell::new(false),
            enable_loro: Cell::new(true),
            storage: StorageSelector::Turso,
            loro_backend: OnceCell::new(),
            loro_org_idle: OnceCell::new(),
        })
    }

    /// Create a new test environment with an explicit storage substrate
    /// (ADR 0004 Phase 9, part (a)). `StorageSelector::Turso` is identical to
    /// [`new`](Self::new); `StorageSelector::LoroMemory` starts a no-Turso
    /// session (no `BackendEngine`; render reads from a `BlockQuerySource`).
    pub fn new_with_backend(
        runtime: Arc<tokio::runtime::Runtime>,
        storage: StorageSelector,
    ) -> Result<Self> {
        let mut env = Self::new(runtime)?;
        env.storage = storage;
        Ok(env)
    }

    /// The storage substrate this environment starts.
    pub fn storage(&self) -> StorageSelector {
        self.storage
    }

    /// The org root as the sync controller sees it (canonicalized temp dir).
    /// Build every in-memory org path from this — see the `org_root` field.
    pub fn org_root(&self) -> &std::path::Path {
        &self.org_root
    }

    /// Deterministically wait until the FileSyncController has processed every
    /// in-memory file change up to and including `seq` (ADR 0011: pair with
    /// `org_fs.last_change_seq()` right after a write). Panics on timeout —
    /// a controller that never processes a synchronously-delivered change is
    /// a wedged sync loop, not a condition to paper over.
    pub async fn wait_for_org_change_processed(&self, seq: u64, timeout: std::time::Duration) {
        let signal = self
            .org_sync_idle
            .get()
            .or(self.loro_org_idle.get())
            .expect("wait_for_org_change_processed: no org sync loop is running");
        if !signal.wait_for_change_seq(seq, timeout).await {
            panic!(
                "Org sync did not process change seq {} within {:?} (watermark at {})",
                seq,
                timeout,
                signal.processed_change_seq()
            );
        }
    }

    /// The in-memory Loro backend of a running `LoroMemory` session — the
    /// storage adapter to seed / mutate (the no-Turso wiring has no engine
    /// dispatch). `None` for Turso or before `start_app`.
    pub fn loro_backend(&self) -> Option<&Arc<LoroBackend>> {
        self.loro_backend.get()
    }

    /// Create and immediately start (existing behavior for backward compatibility).
    ///
    /// Equivalent to `new()` followed by `start_app(true)`.
    pub async fn new_running(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        let mut env = Self::new(runtime)?;
        env.start_app(true).await?;
        Ok(env)
    }

    /// Write an org file to the temp directory.
    ///
    /// Can be called both before and after `start_app()`.
    /// When called before startup, the file will be synced when the app starts.
    pub async fn write_org_file(&self, filename: &str, content: &str) -> Result<PathBuf> {
        let file_path = self.org_root.join(filename);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            self.org_fs.mkdir_all(parent);
        }

        // The in-memory write fires the change event synchronously on
        // completion (ADR 0011) — no watcher-detection delay needed.
        holon_filesystem::FileSystem::write(self.org_fs.as_ref(), &file_path, content.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write org file: {}", e))?;

        let doc_uri = EntityUri::file(filename);
        self.documents
            .borrow_mut()
            .insert(doc_uri, file_path.clone());

        Ok(file_path)
    }

    /// Write a stale/corrupted .loro file to the temp directory.
    ///
    /// This simulates scenarios where a .loro file exists from a previous run
    /// but is corrupted or empty. The system should detect this and recover.
    ///
    /// Can only be called BEFORE `start_app()`.
    pub async fn write_stale_loro_file(
        &self,
        filename: &str,
        corruption_type: LoroCorruptionType,
    ) -> Result<PathBuf> {
        assert!(
            self.session.get().is_none(),
            "Cannot create stale loro file after app started"
        );

        // Replace .org extension with .loro if present
        let loro_filename = if filename.ends_with(".org") {
            filename.replace(".org", ".loro")
        } else {
            format!("{}.loro", filename)
        };

        let loro_path = self.temp_dir.path().join(&loro_filename);

        let content = match corruption_type {
            LoroCorruptionType::Empty => Vec::new(),
            LoroCorruptionType::Truncated => vec![0x4C, 0x6F, 0x72, 0x6F], // "Loro" prefix but truncated
            LoroCorruptionType::InvalidHeader => vec![0xFF, 0xFE, 0x00, 0x01], // Invalid magic bytes
        };

        tokio::fs::write(&loro_path, &content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write stale loro file: {}", e))?;

        Ok(loro_path)
    }

    /// Enable the fake external MCP provider for the next start_app() call.
    ///
    /// When enabled, start_app() registers an in-memory MCP provider that adds
    /// concurrent DDL (cache-table creation) and an initial sync during startup.
    /// This widens the race window and exercises the real external-provider DI
    /// path.
    pub fn set_enable_fake_mcp(&self, enable: bool) {
        self.enable_fake_mcp.set(enable);
    }

    /// Set whether to enable Loro CRDT layer for the next start_app() call.
    pub fn set_enable_loro(&self, enable: bool) {
        self.enable_loro.set(enable);
    }

    /// Whether Loro is enabled for this environment.
    pub fn loro_enabled(&self) -> bool {
        self.enable_loro.get()
    }

    /// The DI-resolved Loro doc store of a running Loro-enabled Turso session.
    /// `None` when Loro is disabled or before `start_app`. Lets tests inspect
    /// the authoritative Loro tree directly (e.g. assert a SQL-only block has
    /// no tree node).
    pub fn loro_doc_store(&self) -> Option<&Arc<RwLock<LoroDocumentStore>>> {
        self.loro_doc_store.get()
    }

    /// Start the application.
    ///
    /// This triggers sync of any pre-existing files and may race with DDL.
    ///
    /// # Arguments
    /// * `wait_for_ready` - If true, wait for file watcher to be ready before returning
    #[tracing::instrument(skip(self), fields(wait_for_ready), name = "test_env.start_app")]
    pub async fn start_app(&self, wait_for_ready: bool) -> Result<()> {
        assert!(self.session.get().is_none(), "App already started");
        holon_frontend::shadow_builders::register_render_dsl_widget_names();

        if self.storage == StorageSelector::LoroMemory {
            return self.start_app_loro_memory().await;
        }

        let holon_config = HolonConfig {
            db_path: Some(self.temp_dir.path().join("test.db")),
            orgmode: holon_frontend::config::OrgmodeConfig {
                root_directory: Some(self.temp_dir.path().to_path_buf()),
            },
            loro: holon_frontend::config::LoroPreferences {
                enabled: if self.enable_loro.get() {
                    Some(true)
                } else {
                    None
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let config_dir = self.temp_dir.path().to_path_buf();
        let mut session_config = SessionConfig::new(holon_api::UiInfo::permissive());
        if !wait_for_ready {
            session_config = session_config.without_wait();
        }
        let enable_fake_mcp = self.enable_fake_mcp.get();
        let org_fs_for_di = self.org_fs.clone();

        let enable_loro = self.enable_loro.get();
        let (
            session,
            backend_engine,
            (doc_store, reactive_engine, sync_handle, idle_signal, debug_services, injector),
        ) = holon_app::new_from_config_with_di(
            holon_config,
            session_config,
            config_dir,
            std::collections::HashSet::new(),
            move |injector| {
                use holon_frontend::reactive::{BuilderServicesSlot, RenderInterpreterInjectorExt};
                override_org_fs_bindings(injector, &org_fs_for_di);
                let slot = injector.resolve::<BuilderServicesSlot>();
                injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                    slot.0.clone(),
                ));
                holon_mcp::di::register_debug_services(injector);
                if enable_fake_mcp {
                    crate::fake_mcp_module::register_fake_mcp(injector);
                }
                Ok(())
            },
            move |injector| {
                use holon_frontend::reactive::{
                    BuilderServices, BuilderServicesSlot, ReactiveEngine,
                };
                let engine = injector.resolve::<ReactiveEngine>();
                let slot = injector.resolve::<BuilderServicesSlot>();
                let services: Arc<dyn BuilderServices> = engine.clone();
                slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent

                let doc_store = if enable_loro {
                    injector
                        .try_resolve::<LoroDocumentStore>()
                        .ok() // ALLOW(ok): optional DI service
                        .map(|store| Arc::new(RwLock::new((*store).clone())))
                } else {
                    None
                };
                let sync_handle = if enable_loro {
                    injector
                        .try_resolve::<holon::sync::LoroSyncControllerHandle>()
                        .ok()
                } else {
                    None
                };
                let idle_signal = injector
                    .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                    .ok();
                let debug_services = populate_debug_services(injector);
                (
                    doc_store,
                    engine,
                    sync_handle,
                    idle_signal,
                    debug_services,
                    injector.clone(),
                )
            },
        )
        .await?;

        let ctx = E2ETestContext::from_engine(backend_engine);

        self.latch_session(session);
        self.latch_injector(injector);
        if let Some(doc_store) = doc_store {
            self.latch_loro_doc_store(doc_store);
        }
        self.latch_debug_services(debug_services);
        if let Some(sync_handle) = sync_handle {
            self.latch_loro_sync_handle(sync_handle);
        }
        self.latch_reactive_engine(reactive_engine);
        if let Some(idle_signal) = idle_signal {
            self.latch_org_sync_idle(idle_signal);
        }
        self.latch_ctx(ctx);

        Ok(())
    }

    /// Stop the running application so a fresh [`Self::start_app`] can reopen
    /// the SAME on-disk `test.db` + in-memory org filesystem — the "user
    /// restarts the app with changed config" scenario (e.g. enabling Loro
    /// over an already-populated vault).
    ///
    /// The Turso actor is shut down explicitly: it is fire-and-forget at
    /// spawn and only exits when every `DbHandle` clone drops, so without the
    /// explicit shutdown any surviving clone (background org-sync loop,
    /// reactive-engine Arc cycle) would keep the WAL writer alive and stall
    /// the next open against the 30s busy_timeout.
    pub async fn stop_app(&mut self) -> Result<()> {
        assert!(self.session.get().is_some(), "stop_app: app not started");
        // Drop CDC consumers before the actor goes away.
        self.active_watches.borrow_mut().clear();
        self.watch_queries.borrow_mut().clear();
        self.ui_model.borrow_mut().clear();
        self.region_streams.borrow_mut().clear();
        self.region_data.borrow_mut().clear();
        *self.all_blocks.borrow_mut() = None;
        *self.all_blocks_stream.borrow_mut() = None;
        self.seed_count.set(None);
        if let Some(ctx) = self.ctx.get() {
            ctx.engine()
                .db_handle()
                .shutdown()
                .await
                .map_err(|e| anyhow::anyhow!("stop_app: Turso actor shutdown failed: {e}"))?;
        }
        // `&mut self` here is what lets `OnceCell::take` reset these build-once
        // fields for the rare config-change restart (the `&self` `start_app`
        // re-latches them afterwards).
        self.session.take();
        self.injector.take();
        self.loro_doc_store.take();
        self.debug_services.take();
        self.loro_sync_handle.take();
        self.reactive_engine.take();
        self.org_sync_idle.take();
        self.loro_backend.take();
        self.loro_org_idle.take();
        self.ctx.take();
        Ok(())
    }

    /// Assemble a `LoroMemory` (no-Turso) session entirely through DI
    /// (ADR 0004 Phase 9, part (a)).
    ///
    /// Builds a Turso-free container ([`build_no_turso_container`]), registers
    /// the Loro storage adapter (`register_loro_block_query_source`), the
    /// Loro-native operation engine (`register_loro_operation_engine`), and the
    /// block-query frontend (`register_block_query_frontend`), then **resolves**
    /// `FrontendSession` + `ReactiveEngine` — the same resolve the Turso path
    /// does, just over a container with no `BackendEngine`. The backend choice
    /// is a DI registration, not a hand-built session. The Turso-heavy services
    /// (`DebugServices`, CDC watches, org sync, seed priming) are simply not
    /// registered in this wiring, so those fields stay `None`.
    ///
    /// Reads and writes share a single [`LoroDocumentStore`]: its global doc is
    /// resolved once up front, the read seam snapshots it, and the operation
    /// engine's `LoroBlockOperations` mutates the same `Arc<LoroDocument>`, so a
    /// mutation is immediately visible to the next read.
    async fn start_app_loro_memory(&self) -> Result<()> {
        let storage_dir = self.temp_dir.path().join("loro-memory");
        std::fs::create_dir_all(&storage_dir)
            .map_err(|e| anyhow::anyhow!("create loro-memory dir: {e}"))?;

        let doc_store = LoroDocumentStore::new(storage_dir.clone());
        // Resolve (and cache) the global doc once so reads and writes share it.
        let doc = doc_store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("get_global_doc: {e}"))?;
        let backend = Arc::new(LoroBackend::from_document(doc));
        let shared_store = Arc::new(RwLock::new(doc_store));

        let injector = build_no_turso_container(storage_dir, {
            let backend = backend.clone();
            let shared_store = shared_store.clone();
            let org_fs = self.org_fs.clone();
            let org_root = self.org_root.clone();
            move |injector| {
                use holon_app::loro_seams::{
                    LoroBlockOrdering, LoroBlockReader, LoroDocumentManager,
                };
                register_loro_block_query_source(injector, backend.clone());
                register_loro_operation_engine(injector, shared_store.clone());
                register_block_query_frontend(injector);

                // Org file-sync over the Loro seams — the SAME backend-blind
                // core the Turso path uses (ADR 0004). Register the three seams +
                // alias registrar + config, then the core; resolving the
                // `FileSyncStarted` marker (post-seed, below) spawns the
                // controller. No `spawn_*` call.
                injector.provide::<holon_orgmode::OrgModeConfig>(fluxdi::Provider::root(
                    move |_| {
                        fluxdi::Shared::new(holon_orgmode::OrgModeConfig::new(org_root.clone()))
                    },
                ));
                {
                    let b = backend.clone();
                    injector.provide::<dyn holon_orgmode::traits::BlockReader>(
                        fluxdi::Provider::root(move |_| {
                            Arc::new(LoroBlockReader::new(b.clone()))
                                as Arc<dyn holon_orgmode::traits::BlockReader>
                        }),
                    );
                }
                {
                    let b = backend.clone();
                    injector.provide::<dyn holon_orgmode::traits::DocumentManager>(
                        fluxdi::Provider::root(move |_| {
                            Arc::new(LoroDocumentManager::new(b.clone()))
                                as Arc<dyn holon_orgmode::traits::DocumentManager>
                        }),
                    );
                }
                {
                    let b = backend.clone();
                    injector.provide::<dyn holon_core::block_ordering::BlockOrdering>(
                        fluxdi::Provider::root(move |_| {
                            Arc::new(LoroBlockOrdering::new(b.clone()))
                                as Arc<dyn holon_core::block_ordering::BlockOrdering>
                        }),
                    );
                }
                {
                    let store = shared_store.clone();
                    injector.provide::<dyn holon_orgmode::file_sync_controller::AliasRegistrar>(
                        fluxdi::Provider::root(move |_| {
                            Arc::new(holon_orgmode::di::LoroAliasRegistrar {
                                doc_store: store.clone(),
                            })
                                as Arc<dyn holon_orgmode::file_sync_controller::AliasRegistrar>
                        }),
                    );
                }
                holon_orgmode::di::register_org_file_sync_core(injector)
                    .map_err(|e| anyhow::anyhow!("register_org_file_sync_core: {e}"))?;
                // Force the in-memory org fs over the core's real-disk defaults.
                override_org_fs_bindings(injector, &org_fs);
                Ok(())
            }
        })
        .await?;

        let session = injector.resolve::<FrontendSession>();
        let reactive_engine = injector.resolve::<ReactiveEngine>();
        // Populate the OnceLock that breaks the engine↔interpreter cycle — the
        // same step the Turso path performs after resolving the engine.
        let slot = injector.resolve::<BuilderServicesSlot>();
        let services: Arc<dyn BuilderServices> = reactive_engine.clone();
        slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent

        self.latch_injector((*injector).clone());
        self.latch_session(session);
        self.latch_reactive_engine(reactive_engine);

        // Seed the default layout (journals page, `__default__`, the bundled
        // index.org root-layout/sidebars) as Block instances written straight
        // into the Loro main storage — the no-Turso analog of
        // `FrontendSession::seed_default_layout`, which the Turso path runs in
        // its session factory. Without this the SUT lacks `block:journals` and
        // the layout blocks the reference seeds at StartApp.
        {
            use holon::api::repository::CoreOperations;
            for block in holon_frontend::FrontendSession::<()>::build_default_layout_blocks(true)? {
                backend
                    .create_block(
                        block.parent_id.clone(),
                        block.to_block_content(),
                        Some(block.id.clone()),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("seed create_block({}): {e}", block.id))?;
                if !block.tags.is_empty() {
                    backend
                        .set_block_tags(block.id.as_str(), &block.tags.to_vec())
                        .await
                        .map_err(|e| anyhow::anyhow!("seed set_block_tags({}): {e}", block.id))?;
                }
                if !block.requires.is_empty() {
                    backend
                        .set_block_requires(block.id.as_str(), &block.requires)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!("seed set_block_requires({}): {e}", block.id)
                        })?;
                }
                if !block.properties.is_empty() {
                    backend
                        .update_block_properties(block.id.as_str(), &block.properties)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!("seed update_block_properties({}): {e}", block.id)
                        })?;
                }
            }
        }

        // Start org sync by resolving the backend-blind FileSyncStarted marker
        // (registered above via register_org_file_sync_core). Same self-starting
        // DI path the Turso container uses — no `spawn_*` call, no hardcoded
        // adapters. Done AFTER seeding so the initial scan sees a seeded vault.
        injector
            .resolve_async::<holon_orgmode::di::FileSyncStarted>()
            .await;
        let mut org_ready = (*injector.resolve::<holon_orgmode::FileWatcherReadySignal>())
            .clone()
            .into_receiver();
        org_ready
            .wait_for(|v| v.is_some())
            .await
            .expect("FileWatcherReadySignal sender dropped");
        match org_ready.borrow().as_ref().unwrap() {
            Ok(()) => {}
            Err(msg) => anyhow::bail!("no-Turso org sync startup failed: {msg}"),
        }
        self.latch_loro_org_idle(injector.resolve::<holon_orgmode::OrgSyncIdleSignal>());

        self.latch_loro_backend(backend);
        self.latch_loro_doc_store(shared_store);
        Ok(())
    }

    /// Check if app is running
    pub fn is_running(&self) -> bool {
        self.session.get().is_some()
    }

    /// Get the running session (panics if not started)
    pub fn session(&self) -> &FrontendSession {
        self.session
            .get()
            .expect("App not started - call start_app() first")
    }

    /// Get the running session as an Arc (panics if not started)
    pub fn session_arc(&self) -> Arc<FrontendSession> {
        Arc::clone(
            self.session
                .get()
                .expect("App not started - call start_app() first"),
        )
    }

    /// Get the E2ETestContext (panics if not started)
    ///
    /// Use this for direct access to the test context operations.
    pub fn test_ctx(&self) -> &E2ETestContext {
        self.ctx
            .get()
            .expect("App not started - call start_app() first")
    }

    /// Latch a build-once session field via `&self`. Fail-loud if already set
    /// (would mean `start_app` ran twice without an intervening `stop_app`).
    fn latch_session(&self, value: Arc<FrontendSession>) {
        self.session
            .set(value)
            .unwrap_or_else(|_| panic!("session already latched (start_app ran twice?)"));
    }

    fn latch_injector(&self, value: fluxdi::Injector) {
        self.injector
            .set(value)
            .unwrap_or_else(|_| panic!("injector already latched (start_app ran twice?)"));
    }

    fn latch_loro_doc_store(&self, value: Arc<RwLock<LoroDocumentStore>>) {
        self.loro_doc_store
            .set(value)
            .unwrap_or_else(|_| panic!("loro_doc_store already latched (start_app ran twice?)"));
    }

    fn latch_debug_services(&self, value: Arc<holon_mcp::server::DebugServices>) {
        self.debug_services
            .set(value)
            .unwrap_or_else(|_| panic!("debug_services already latched (start_app ran twice?)"));
    }

    fn latch_loro_sync_handle(&self, value: Arc<holon::sync::LoroSyncControllerHandle>) {
        self.loro_sync_handle
            .set(value)
            .unwrap_or_else(|_| panic!("loro_sync_handle already latched (start_app ran twice?)"));
    }

    fn latch_reactive_engine(&self, value: Arc<holon_frontend::reactive::ReactiveEngine>) {
        self.reactive_engine
            .set(value)
            .unwrap_or_else(|_| panic!("reactive_engine already latched (start_app ran twice?)"));
    }

    fn latch_org_sync_idle(&self, value: Arc<holon_orgmode::OrgSyncIdleSignal>) {
        self.org_sync_idle
            .set(value)
            .unwrap_or_else(|_| panic!("org_sync_idle already latched (start_app ran twice?)"));
    }

    fn latch_ctx(&self, value: E2ETestContext) {
        self.ctx
            .set(value)
            .unwrap_or_else(|_| panic!("ctx already latched (start_app ran twice?)"));
    }

    fn latch_loro_backend(&self, value: Arc<LoroBackend>) {
        self.loro_backend
            .set(value)
            .unwrap_or_else(|_| panic!("loro_backend already latched (start_app ran twice?)"));
    }

    fn latch_loro_org_idle(&self, value: std::sync::Arc<holon_orgmode::di::OrgSyncIdleSignal>) {
        self.loro_org_idle
            .set(value)
            .unwrap_or_else(|_| panic!("loro_org_idle already latched (start_app ran twice?)"));
    }

    /// Check for startup errors (delegates to FrontendSession)
    pub fn has_startup_errors(&self) -> bool {
        self.session().has_startup_errors()
    }

    /// Get the number of publish errors that occurred
    pub fn startup_error_count(&self) -> usize {
        self.session().startup_error_count()
    }

    /// Get the publish error tracker for monitoring startup errors
    pub fn publish_error_tracker(&self) -> &PublishErrorTracker {
        self.session().error_tracker()
    }

    /// Get the underlying engine (requires running app).
    ///
    /// Sourced from the `E2ETestContext` captured at startup, not from
    /// `FrontendSession` (which no longer stores the engine — ADR 0004 Phase 9).
    pub fn engine(&self) -> &Arc<BackendEngine> {
        self.test_ctx().engine()
    }

    /// Get the doc store (requires running app with Loro enabled).
    /// Returns None when Loro is disabled.
    pub fn doc_store(&self) -> Option<&Arc<RwLock<LoroDocumentStore>>> {
        self.loro_doc_store.get()
    }

    /// DI-resolved + populated `DebugServices` for the embedded MCP
    /// server. `None` before `start_app()` runs.
    pub fn debug_services(&self) -> Option<&Arc<holon_mcp::server::DebugServices>> {
        self.debug_services.get()
    }

    /// The `LoroSyncController` handle, if Loro is enabled. Shared into
    /// `LoroSut` so peer-sync ops can wait for reactive quiescence.
    pub fn loro_sync_handle(&self) -> Option<&Arc<holon::sync::LoroSyncControllerHandle>> {
        self.loro_sync_handle.get()
    }

    /// Number of errors logged by the `LoroSyncController` since startup.
    /// Returns 0 when Loro is disabled (handle is None).
    pub fn loro_sync_error_count(&self) -> usize {
        self.loro_sync_handle
            .get()
            .map(|h| h.error_count())
            .unwrap_or(0)
    }

    /// Wait until the Turso CDC emission watermark stops advancing for
    /// `quiet_for`, bounded by `timeout`.
    ///
    /// A watermark-free settle barrier (replaces the former
    /// `wait_for_consumers` event-bus ack-watermark wait — that per-consumer
    /// ack watermark was test-only scaffolding). It proves every matview CDC
    /// batch the latest transition produced — block, region, **and
    /// file/directory** — has been *emitted* before the caller samples
    /// `target_seq` in [`Self::assert_cdc_quiescent`], so legitimate
    /// file/directory CDC isn't mistaken for post-settlement churn. It reads
    /// the same `cdc_emitted_watermark` the quiescence assert itself trusts.
    ///
    /// Best-effort: returns on timeout (the hard gate is `assert_cdc_quiescent`).
    /// No-op when the app isn't running (pre-`StartApp` transitions).
    pub async fn wait_for_cdc_quiescent(
        &self,
        quiet_for: std::time::Duration,
        timeout: std::time::Duration,
    ) {
        let Some(ctx) = self.ctx.get() else { return };
        let start = tokio::time::Instant::now();
        let mut last = ctx.engine().db_handle().cdc_emitted_watermark();
        let mut stable_since = tokio::time::Instant::now();
        loop {
            if start.elapsed() >= timeout {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            let now = ctx.engine().db_handle().cdc_emitted_watermark();
            if now == last {
                if stable_since.elapsed() >= quiet_for {
                    return;
                }
            } else {
                last = now;
                stable_since = tokio::time::Instant::now();
            }
        }
    }

    /// Wait for the `LoroSyncController` to reach quiescence — i.e., its
    /// `last_synced` watermark matches the current `oplog_frontiers()`.
    /// No-op when Loro is disabled.
    pub async fn wait_for_loro_quiescence(&self, timeout: std::time::Duration) {
        let (Some(handle), Some(doc_store)) =
            (self.loro_sync_handle.get(), self.loro_doc_store.get())
        else {
            return;
        };
        wait_for_loro_quiescence_on(handle, doc_store, timeout).await;
    }

    /// Create an org file in the temp directory (requires running app).
    ///
    /// When Loro is enabled, also loads the file into the LoroDocumentStore.
    /// When Loro is disabled, just writes the file and tracks it.
    pub async fn create_document(&self, file_name: &str) -> Result<EntityUri> {
        let file_path = self.org_root.join(file_name);
        holon_filesystem::FileSystem::write(self.org_fs.as_ref(), &file_path, b"")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create org file: {}", e))?;

        if let Some(doc_store) = self.doc_store() {
            let mut store = doc_store.write().await;
            store
                .get_or_load(&file_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to load org file: {}", e))?;
        }

        // Wait for FileSyncController to create the document entity with a UUID.
        // `resolve_doc_uri_by_name` is backend-agnostic (Turso `block_raw` or
        // the Loro `BlockQuerySource` snapshot), so this poll covers both
        // wirings as the controller's watcher ingests the new file.
        let timeout = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();
        let doc_uri = loop {
            if let Ok(uri) = self.resolve_doc_uri_by_name(file_name).await {
                break uri;
            }
            assert!(
                start.elapsed() < timeout,
                "Timeout waiting for document entity for '{}'",
                file_name
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        self.documents
            .borrow_mut()
            .insert(doc_uri.clone(), file_path);

        Ok(doc_uri)
    }

    /// Execute an operation on the backend
    pub async fn execute_operation(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let params: holon_api::StorageEntity = params
            .into_iter()
            .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v))
            .collect();
        self.test_ctx().execute_op(entity, op, params).await
    }

    /// Query the backend
    pub async fn query(
        &self,
        source: &str,
        language: QueryLanguage,
    ) -> Result<Vec<holon_api::widget_spec::DataRow>> {
        self.test_ctx()
            .query(source.to_string(), language, HashMap::new())
            .await
    }

    /// Resolve a file-based document URI (e.g. "doc:doc_0.org") to the real
    /// UUID-based URI used by the system.
    pub async fn resolve_doc_uri(&self, file_uri: &EntityUri) -> Result<EntityUri> {
        let path_part = file_uri.id();

        let name = std::path::Path::new(path_part)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Cannot extract name from URI: {}", file_uri))?;

        let sql = format!(
            "SELECT b.id FROM block_raw b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' \
             AND substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) = '{}'",
            name
        );
        let rows = self.query_sql(&sql).await?;
        let id = rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No document found with name '{}' (from URI '{}')",
                    name,
                    file_uri
                )
            })?;
        EntityUri::parse(id)
    }

    /// Resolve a document by filename (e.g. "index.org") to its `block:uuid` URI.
    pub async fn resolve_doc_uri_by_name(&self, filename: &str) -> Result<EntityUri> {
        let name = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Cannot extract stem from filename: {}", filename))?;

        // No-Turso: there is no `block_raw` table — read the page block straight
        // from the session's `BlockQuerySource` snapshot (the Loro tree). A page
        // is `is_page()` and its title is the first line of `content`, matching
        // the Turso SQL below. This is the read-side mirror of the org→Loro
        // ingest just performed by `FileSyncController`, so it also verifies the
        // document actually landed (not an assumed identity).
        if matches!(self.storage, StorageSelector::LoroMemory) {
            let session = self
                .session
                .get()
                .ok_or_else(|| anyhow::anyhow!("resolve_doc_uri_by_name: app not started"))?;
            let snapshot = session
                .block_query()
                .snapshot()
                .await
                .map_err(|e| anyhow::anyhow!("block_query snapshot failed: {e}"))?;
            let page = snapshot
                .iter_blocks()
                .find(|b| b.is_page() && b.title() == name)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No document found with name '{}' (from filename '{}') in Loro snapshot",
                        name,
                        filename
                    )
                })?;
            return Ok(page.id.clone());
        }

        let sql = format!(
            "SELECT b.id FROM block_raw b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' \
             AND substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) = '{}'",
            name
        );
        let rows = self.query_sql(&sql).await?;
        let id = rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No document found with name '{}' (from filename '{}')",
                    name,
                    filename
                )
            })?;
        EntityUri::parse(id)
    }

    /// Execute a raw SQL query and return rows.
    pub async fn query_sql(&self, sql: &str) -> Result<Vec<holon_api::widget_spec::DataRow>> {
        self.query(sql, QueryLanguage::HolonSql).await
    }

    /// Non-page content block rows as `{id, parent_id, sort_key}`, read
    /// backend-agnostically: Turso queries `block_raw`; no-Turso reads the
    /// `BlockQuerySource` snapshot (the Loro tree, already in document order) and
    /// synthesizes a zero-padded `sort_key` from that order so callers that sort
    /// by it preserve sibling order. Used by the SplitBlock/JoinBlock
    /// reconciliation that the `{Loro}` slice exercises.
    pub async fn non_page_block_rows(&self) -> Vec<holon_api::StorageEntity> {
        if matches!(self.storage, StorageSelector::LoroMemory) {
            let session = self
                .session
                .get()
                .expect("non_page_block_rows: app not started");
            let snapshot = session
                .block_query()
                .snapshot()
                .await
                .expect("non_page_block_rows: block_query snapshot failed");
            return snapshot
                .iter_blocks()
                .filter(|b| !b.is_page())
                .enumerate()
                .map(|(i, b)| {
                    let mut row: holon_api::StorageEntity = HashMap::new();
                    row.insert("id".into(), Value::String(b.id.to_string()));
                    row.insert("parent_id".into(), Value::String(b.parent_id.to_string()));
                    row.insert("sort_key".into(), Value::String(format!("{i:08}")));
                    row
                })
                .collect();
        }
        let sql = "SELECT id, parent_id, sort_key FROM block_raw \
                   WHERE id NOT IN (SELECT block_id FROM block_tags WHERE tag = 'Page')"
            .to_string();
        self.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("non_page_block_rows: block_raw query failed")
    }

    /// Watch a block's UI and wait for the first Structure event.
    ///
    /// Returns the RenderExpr from the first Structure event, plus the WatchHandle
    /// for further interaction.
    pub async fn watch_ui_first_structure(
        &self,
        block_id: &EntityUri,
    ) -> Result<(RenderExpr, holon_api::WatchHandle)> {
        let engine = self.engine();
        let mut watch = holon::api::watch_ui(Arc::clone(engine), block_id.clone()).await?;

        // Wait for the first Structure event
        let render_expr = loop {
            let event = watch
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("watch_ui stream closed before Structure event"))?;
            if let holon_api::UiEvent::Structure { render_expr, .. } = event {
                break render_expr;
            }
        };

        Ok((render_expr, watch))
    }

    /// Wait for the next Structure event on a watch_ui stream.
    pub async fn wait_for_next_structure(
        watch: &mut holon_api::WatchHandle,
        timeout: std::time::Duration,
    ) -> Result<RenderExpr> {
        let deadline = tokio::time::timeout(timeout, async {
            loop {
                let event = watch
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watch_ui stream closed"))?;
                if let holon_api::UiEvent::Structure { render_expr, .. } = event {
                    return Ok::<_, anyhow::Error>(render_expr);
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("Timed out waiting for Structure event"))??;

        Ok(deadline)
    }

    /// Get path to an org file
    pub fn org_file_path(&self, file_name: &str) -> PathBuf {
        self.org_root.join(file_name)
    }

    /// Get the temp directory path
    pub fn temp_path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    /// Get path to a document by doc_uri
    pub fn get_document_path(&self, doc_uri: &EntityUri) -> Option<PathBuf> {
        self.documents.borrow().get(doc_uri).cloned()
    }

    /// Reload an org file from disk (removes from store and re-loads).
    /// Only meaningful when Loro is enabled; no-op otherwise.
    pub async fn reload_org_file(&self, file_path: &PathBuf) -> Result<()> {
        if let Some(doc_store) = self.doc_store() {
            let mut store = doc_store.write().await;
            store.remove(file_path).await;
            store
                .get_or_load(file_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to reload org file: {}", e))?;
        }
        Ok(())
    }

    /// Call initial_widget and return both the render expression and CDC stream.
    ///
    /// Render the root layout block, returning RenderExpr + CDC stream.
    pub async fn initial_widget_with_stream(&self) -> Result<(RenderExpr, RowChangeStream)> {
        self.engine()
            .blocks()
            .render_entity(&holon_api::root_layout_block_uri(), &None)
            .await
    }

    /// Render the root layout block (discards stream).
    pub async fn initial_widget(&self) -> Result<RenderExpr> {
        let (render_expr, _stream) = self.initial_widget_with_stream().await?;
        Ok(render_expr)
    }

    /// Call initial_widget and recursively render all nested PRQL blocks.
    ///
    /// This simulates what the Flutter UI does:
    /// 1. Call initial_widget to get the root layout
    /// 2. Query root layout children directly
    /// 3. For each row that is a PRQL source block, execute its query with parent context
    /// 4. Collect all rendered data
    ///
    /// Returns the root RenderExpr and combined data from all rendered panels.
    pub async fn initial_widget_fully_rendered(
        &self,
    ) -> Result<(RenderExpr, Vec<holon_api::widget_spec::DataRow>)> {
        use holon_api::widget_spec::DataRow;

        let (root_render_expr, _stream) = self.initial_widget_with_stream().await?;

        // Get root layout children via execute_query
        let root_data: Vec<DataRow> = self
            .query(
                "SELECT id, content, content_type, source_language, parent_id FROM block_raw WHERE parent_id = 'block:root-layout' OR id = 'block:root-layout'",
                QueryLanguage::HolonSql,
            )
            .await?;

        // Collect all data: start with root layout data
        let mut all_data = root_data.clone();

        // Process each row - if it's a PRQL source block, render it
        for row in &root_data {
            let content_type: Option<ContentType> = row
                .get("content_type")
                .and_then(|v| v.as_string())
                .map(|s| s.parse().expect("Invalid content_type in row"));
            let source_language = row.get("source_language").and_then(|v| v.as_string());

            if content_type == Some(ContentType::Source)
                && let Some(query_lang) = source_language
                    .and_then(|s| s.parse::<SourceLanguage>().ok()) // ALLOW(ok): boundary parse
                    .and_then(|sl| sl.as_query())
            {
                {
                    let block_id = row.get("id").and_then(|v| v.as_string());
                    let parent_id = row
                        .get("parent_id")
                        .and_then(|v| v.as_string())
                        .map(|s| EntityUri::parse(s).expect("valid parent_id URI"));
                    let query_content = row.get("content").and_then(|v| v.as_string());

                    if let (Some(_block_id), Some(parent_id), Some(source)) =
                        (block_id, parent_id, query_content)
                    {
                        match self
                            .query_with_context(source, query_lang, &parent_id)
                            .await
                        {
                            Ok(nested_rows) => {
                                all_data.extend(nested_rows);
                            }
                            Err(e) => {
                                eprintln!(
                                    "[test] Failed to render nested block under {}: {}",
                                    parent_id, e
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok((root_render_expr, all_data))
    }

    /// Execute a query with context (simulating nested render_entity).
    ///
    /// This simulates what the Flutter UI does when it encounters `render_entity this`:
    /// - Takes a query source from a block
    /// - Executes it with the parent block's ID as context for `from children`
    /// Uses FrontendSession directly to ensure identical code path with Flutter.
    ///
    /// # Arguments
    /// * `source` - The query source to execute
    /// * `language` - The query language ("holon_prql", "holon_gql", "holon_sql")
    /// * `context_block_id` - The block ID to use for `from children` resolution
    pub async fn query_with_context(
        &self,
        source: &str,
        language: QueryLanguage,
        context_block_id: &EntityUri,
    ) -> Result<Vec<holon_api::widget_spec::DataRow>> {
        let session = self.session();
        let engine = self.engine();
        let sql = engine.compile_to_sql(source, language)?;
        let block_path = session.lookup_block_path(context_block_id).await?;
        let context = QueryContext::for_block_with_path(context_block_id, None, block_path);
        let rows = engine
            .execute_query(sql, HashMap::new(), Some(context))
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            .collect())
    }

    /// Simulate what the Flutter UI does when rendering a query source block.
    ///
    /// When the UI encounters `render_entity this` for a source block,
    /// it should execute the query with the source block's PARENT as context.
    /// This is because `from children` in that query should get children of
    /// the heading (parent), not children of the source block itself.
    ///
    /// # Arguments
    /// * `source_block_id` - The ID of the source block (e.g., "right_sidebar::src::0")
    ///
    /// # Returns
    /// The data rows from executing the source block's query with parent context
    pub async fn render_source_block(
        &self,
        source_block_id: &str,
    ) -> Result<Vec<holon_api::widget_spec::DataRow>> {
        // First, get the source block to find its content, language, and parent
        let blocks = self
            .engine()
            .execute_query(
                "SELECT parent_id, content, source_language FROM block_raw WHERE id = $id"
                    .to_string(),
                {
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(source_block_id.to_string()));
                    params
                },
                None,
            )
            .await?;

        let block = blocks
            .first()
            .ok_or_else(|| anyhow::anyhow!("Source block '{}' not found", source_block_id))?;

        let parent_id = block
            .get("parent_id")
            .and_then(|v| v.as_string())
            .map(|s| EntityUri::parse(s).expect("valid parent_id URI"))
            .ok_or_else(|| anyhow::anyhow!("Source block has no parent_id"))?;

        let content = block
            .get("content")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("Source block has no content"))?;

        let language: QueryLanguage = block
            .get("source_language")
            .and_then(|v| v.as_string())
            .map(|s| s.parse::<SourceLanguage>())
            .transpose()
            .expect("Invalid source_language in block")
            .and_then(|sl| sl.as_query())
            .expect("Source block's language is not a query language");

        // Execute the query with the PARENT's context (not the source block's own ID)
        self.query_with_context(content, language, &parent_id).await
    }

    /// Create a document and wait for the external_processing window to close.
    ///
    /// This is useful for PBT tests that need to ensure the file watcher has
    /// fully processed the new document before proceeding.
    pub async fn create_document_with_sync_wait(&mut self, file_name: &str) -> Result<EntityUri> {
        let doc_uri = self.create_document(file_name).await?;
        self.wait_for_org_files_stable(25, std::time::Duration::from_millis(5000))
            .await;
        Ok(doc_uri)
    }

    /// Pre-drain delivery barrier shared by `drain_cdc_events` /
    /// `drain_region_cdc_events`: wait for the Turso CDC emission watermark
    /// to go quiet (5 ms stable, 500 ms cap) so the subsequent non-blocking
    /// poll doesn't race mid-emission producers. Without a Turso ctx
    /// (no-Turso wiring / pre-StartApp) fall back to the former flat 5 ms —
    /// there is no watermark to consult, and producers (Loro sync) still
    /// need real wall time.
    pub async fn drain_delivery_barrier(&self) {
        if self.ctx.get().is_some() {
            self.wait_for_cdc_quiescent(
                std::time::Duration::from_millis(5),
                std::time::Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }
    }

    /// Drain CDC events from all active watches and update ui_model.
    #[tracing::instrument(skip(self), name = "pbt.drain_cdc_events")]
    pub async fn drain_cdc_events(&mut self) {
        use futures::FutureExt;

        // Drain CDC events without blocking: barrier on the CDC emission
        // watermark (the same signal `assert_cdc_quiescent` trusts) instead
        // of a flat 5 ms sleep, so we don't poll while producers are
        // mid-emission and don't flake when they need longer. Then
        // `now_or_never` every poll so an empty channel exits immediately.
        // No-Turso (no ctx) keeps the former short sleep — the watermark is
        // Turso-side, and pure `yield_now` caused Loro quiescence races in
        // the cross-executor PBT variant.
        //
        // Correctness gate: inv-backend-blocks-match-ref (SQL = ref), inv-watch-rows-match-ref (UI model = ref), and inv-region-focus-roots
        // (region focus roots) all start failing if the producer hasn't
        // actually delivered events by the time we poll.
        self.drain_delivery_barrier().await;

        // Hold the `active_watches` borrow for the loop; borrow `ui_model`
        // (a distinct cell) inside each hit. No `.await` in this loop, so the
        // guards never cross a suspension point; both drop before the
        // `all_blocks_stream` (`&mut self`) access below.
        for (query_id, stream) in self.active_watches.borrow_mut().iter_mut() {
            let mut event_count = 0;
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        event_count += batch.inner.items.len();
                        if let Some(ui_data) = self.ui_model.borrow_mut().get_mut(query_id) {
                            for change in &batch.inner.items {
                                if let holon_api::Change::Updated { id, data, .. } = &change.change
                                    && let Some(content) =
                                        data.get("content").and_then(|v| v.as_string())
                                {
                                    eprintln!(
                                        "[drain_cdc] watch '{}': Updated id={} content={:?}",
                                        query_id, id, content
                                    );
                                }
                                ui_data.apply_change(rekey_change(change.change.clone()));
                            }
                        }
                    }
                    Some(None) => break, // stream closed
                    None => break,       // nothing immediately ready
                }
            }
            if event_count > 0 {
                eprintln!(
                    "[drain_cdc] watch '{}': drained {} CDC events",
                    query_id, event_count
                );
            }
        }

        if let (Some(stream), Some(acc)) = (
            self.all_blocks_stream.borrow_mut().as_mut(),
            self.all_blocks.borrow_mut().as_mut(),
        ) {
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        for change in batch.inner.items {
                            acc.apply_change(rekey_change(change.change));
                        }
                    }
                    _ => break,
                }
            }
        }
    }

    /// Drain CDC events from all region streams and update region_data.
    #[tracing::instrument(skip(self), name = "pbt.drain_region_cdc_events")]
    pub async fn drain_region_cdc_events(&mut self) {
        use futures::FutureExt;

        self.drain_delivery_barrier().await;

        // No `.await` inside the loop (`now_or_never` polls synchronously), so
        // holding both RefCell borrows across it is sound.
        let mut region_streams = self.region_streams.borrow_mut();
        let mut region_data = self.region_data.borrow_mut();
        for (region_id, stream) in region_streams.iter_mut() {
            let mut event_count = 0;
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        event_count += batch.inner.items.len();
                        if let Some(region_data) = region_data.get_mut(region_id) {
                            for change in &batch.inner.items {
                                region_data.apply_change(rekey_change(change.change.clone()));
                            }
                        }
                    }
                    _ => break,
                }
            }
            if event_count > 0 {
                tracing::trace!(
                    "[drain_region_cdc] region '{}': drained {} CDC events",
                    region_id,
                    event_count
                );
            }
        }
    }

    /// Assert no spurious CDC events arrive after the system has settled.
    ///
    /// Called after `drain_cdc_events` + `drain_region_cdc_events`. Sleeps to
    /// give producers real wall time, then polls all CDC streams. Any event
    /// arriving after settlement indicates the backend is churning — emitting
    /// add/remove cycles for data that hasn't actually changed.
    pub async fn assert_cdc_quiescent(&mut self) {
        use futures::FutureExt;

        if self.active_watches.borrow().is_empty()
            && self.region_streams.borrow().is_empty()
            && self.all_blocks_stream.borrow().is_none()
        {
            return;
        }

        // Sample the global CDC emission watermark BEFORE polling. Turso's
        // IVM is synchronous within commit, so by the time `apply_transition`
        // returned, the change-callback has already run and stamped each
        // batch with a monotonic `seq`. Anything stamped with `seq <= target`
        // is "expected output of the transition". A batch with `seq > target`
        // arriving during the wait IS the bug we want to assert against.
        let target_seq = self
            .ctx
            .get()
            .map(|c| c.engine().db_handle().cdc_emitted_watermark())
            .unwrap_or(0);

        // Quiescence-with-budget. The guard exists to catch a *churning*
        // backend — one emitting add/remove cycles for data that hasn't
        // changed, never reaching a fixed point. The previous rule ("fail on
        // any batch with seq > target_seq seen within a 50 ms drain") also
        // tripped on a *benign convergence tail*: a single late,
        // file-watcher-driven re-projection (`on_file_changed` firing one
        // extra `block` UPDATE just after the per-transition settle sampled
        // `target_seq`). That tail fires once and stops; real churn does not.
        // So we decide on whether post-`target_seq` activity SETTLES, not on
        // whether it occurs at all: pass once it has been quiet for
        // `quiet_for`, fail only if it keeps arriving past `budget`.
        let quiet_for = tokio::time::Duration::from_millis(150);
        let budget = tokio::time::Duration::from_secs(2);
        // Common (quiescent) case: how long to keep draining while streams
        // are still catching up to `target_seq` before giving up. Matches the
        // shared quiescence floor (`pbt_quiet_floor`, default 50ms) so churn-
        // free transitions stay fast and tune together when probing the floor.
        let catchup_grace = pbt_quiet_floor();
        let started = tokio::time::Instant::now();
        // When we last observed a batch with `seq > target_seq`. `None` until
        // the first post-target batch arrives.
        let mut last_post_target: Option<tokio::time::Instant> = None;
        let mut spurious: Vec<(String, usize)> = Vec::new();
        // For each spurious source, keep a compact one-line summary of every
        // change record so a failure dump shows what actually leaked, not
        // just the count.
        let mut spurious_dump: Vec<(String, u64, String)> = Vec::new();
        let mut watch_seen: HashMap<String, u64> = HashMap::new();
        let mut region_seen: HashMap<String, u64> = HashMap::new();
        let mut all_blocks_seen: u64 = 0;

        loop {
            let mut still_pending = false;

            for (query_id, stream) in self.active_watches.borrow_mut().iter_mut() {
                let mut count = 0usize;
                while let Some(Some(batch)) = stream.next().now_or_never() {
                    let batch_seq = batch.metadata.seq;
                    let known_seq = watch_seen.entry(query_id.clone()).or_insert(0);
                    *known_seq = (*known_seq).max(batch_seq);
                    if batch_seq > target_seq {
                        count += batch.inner.items.len();
                        last_post_target = Some(tokio::time::Instant::now());
                        for change in &batch.inner.items {
                            spurious_dump.push((
                                format!("watch:{query_id}"),
                                batch_seq,
                                summarize_change(&rekey_change(change.change.clone())),
                            ));
                        }
                    }
                    if let Some(ui_data) = self.ui_model.borrow_mut().get_mut(query_id) {
                        for change in &batch.inner.items {
                            ui_data.apply_change(rekey_change(change.change.clone()));
                        }
                    }
                }
                if count > 0 {
                    spurious.push((format!("watch:{query_id}"), count));
                }
                if watch_seen.get(query_id).copied().unwrap_or(0) < target_seq {
                    still_pending = true;
                }
            }

            {
                // No `.await` in this loop (`now_or_never`), so holding both
                // RefCell borrows across it is sound.
                let mut region_streams = self.region_streams.borrow_mut();
                let mut region_data = self.region_data.borrow_mut();
                for (region_id, stream) in region_streams.iter_mut() {
                    let mut count = 0usize;
                    while let Some(Some(batch)) = stream.next().now_or_never() {
                        let batch_seq = batch.metadata.seq;
                        let known_seq = region_seen.entry(region_id.clone()).or_insert(0);
                        *known_seq = (*known_seq).max(batch_seq);
                        if batch_seq > target_seq {
                            count += batch.inner.items.len();
                            last_post_target = Some(tokio::time::Instant::now());
                            for change in &batch.inner.items {
                                spurious_dump.push((
                                    format!("region:{region_id}"),
                                    batch_seq,
                                    summarize_change(&rekey_change(change.change.clone())),
                                ));
                            }
                        }
                        if let Some(region_data) = region_data.get_mut(region_id) {
                            for change in &batch.inner.items {
                                region_data.apply_change(rekey_change(change.change.clone()));
                            }
                        }
                    }
                    if count > 0 {
                        spurious.push((format!("region:{region_id}"), count));
                    }
                    if region_seen.get(region_id).copied().unwrap_or(0) < target_seq {
                        still_pending = true;
                    }
                }
            }

            if let (Some(stream), Some(acc)) = (
                self.all_blocks_stream.borrow_mut().as_mut(),
                self.all_blocks.borrow_mut().as_mut(),
            ) {
                let mut count = 0usize;
                while let Some(Some(batch)) = stream.next().now_or_never() {
                    let batch_seq = batch.metadata.seq;
                    all_blocks_seen = all_blocks_seen.max(batch_seq);
                    if batch_seq > target_seq {
                        count += batch.inner.items.len();
                        last_post_target = Some(tokio::time::Instant::now());
                        for change in &batch.inner.items {
                            spurious_dump.push((
                                "all_blocks".to_string(),
                                batch_seq,
                                summarize_change(&rekey_change(change.change.clone())),
                            ));
                        }
                    }
                    for change in batch.inner.items {
                        acc.apply_change(rekey_change(change.change));
                    }
                }
                if count > 0 {
                    spurious.push(("all_blocks".to_string(), count));
                }
                if all_blocks_seen < target_seq {
                    still_pending = true;
                }
            }

            match last_post_target {
                // No post-`target_seq` activity yet — the common, quiescent
                // case. Exit as soon as every stream caught up to the
                // watermark, or after a short grace if some stream simply
                // emitted nothing this transition.
                None => {
                    if !still_pending || started.elapsed() >= catchup_grace {
                        break;
                    }
                }
                // A post-target batch arrived. Keep draining until it has
                // been quiet for `quiet_for` (settled → benign tail) or the
                // overall `budget` expires (still churning → real bug).
                Some(last) => {
                    if last.elapsed() >= quiet_for || started.elapsed() >= budget {
                        break;
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
        }

        // Post-target activity is fatal only if it never settled — i.e. the
        // most recent post-target batch is still within `quiet_for` of the
        // budget cutoff (it was still arriving when we gave up). A tail that
        // went quiet is a disclosed-but-benign file-sync convergence echo.
        let churned = last_post_target
            .map(|last| last.elapsed() < quiet_for)
            .unwrap_or(false);

        if churned {
            eprintln!(
                "[inv-editable-text-has-draggable] CDC not quiescent — post-watermark({target_seq}) events kept arriving past {budget:?}: {:?}",
                spurious,
            );
            // Dump every leaked change so the panic log is enough to
            // identify which writes are firing CDC, without needing MCP
            // attachment or sqlite inspection.
            eprintln!(
                "[inv-editable-text-has-draggable] churning change records (source, seq, change):"
            );
            for (source, seq, summary) in &spurious_dump {
                eprintln!("    [{source} seq={seq}] {summary}");
            }
        } else if !spurious.is_empty() {
            // Benign tail: one or more late re-projections that quiesced
            // within `quiet_for`. Surface it (don't hide it) but don't flake.
            eprintln!(
                "[inv-editable-text-has-draggable] NOTE: post-settlement CDC tail settled within {quiet_for:?} \
                 (benign file-sync convergence echo, not churn): {:?}",
                spurious,
            );
        }

        assert!(
            !churned,
            "[inv-editable-text-has-draggable] CDC not quiescent: backend kept churning past {budget:?} \
             (post-settlement events still arriving): {:?}. This indicates the backend is \
             emitting add/remove cycles for unchanged data.",
            spurious,
        );
    }

    /// Parse all Org files in the temp directory and return blocks.
    ///
    /// Uses the production `Block` struct for accurate testing.
    /// Parse all Org files in the temp directory and return blocks.
    ///
    /// If `todo_header` is provided (e.g. `"#+TODO: STARTED | DONE CANCELLED"`),
    /// it is prepended to each file's content before parsing so the parser
    /// recognizes custom keywords — matching how production FileSyncController
    /// stores keywords on the Document entity.
    pub async fn parse_org_file_blocks(&self, todo_header: Option<&str>) -> Result<Vec<Block>> {
        use holon_orgmode::parser::parse_org_file;

        let mut all_blocks = Vec::new();
        let root = self.org_root.as_path();

        let file_paths: Vec<PathBuf> = self.documents.borrow().values().cloned().collect();
        for file_path in &file_paths {
            let raw = self.org_fs.read_to_string(file_path).await?;
            let content = match todo_header {
                Some(header) if !raw.contains("#+TODO:") => format!("{}\n{}", header, raw),
                _ => raw,
            };
            let result = parse_org_file(file_path, &content, &EntityUri::no_parent(), root)?;
            all_blocks.extend(result.blocks);
        }

        Ok(all_blocks)
    }

    /// Set up a CDC-driven region watch that tracks `focus_roots JOIN block`.
    /// When navigation changes `focus_roots` via IVM, CDC propagates to this chained matview.
    pub async fn setup_region_watch(&self, region: Region) -> Result<()> {
        let sql = format!(
            "SELECT fr.root_id AS id, b.content, b.parent_id \
             FROM focus_roots fr \
             JOIN block b ON b.id = fr.root_id \
             WHERE fr.region = '{}'",
            region.as_str()
        );
        let stream = self
            .engine()
            .query_and_watch(sql, HashMap::new(), None)
            .await?;

        let region_key = region.as_str().to_string();
        self.region_data
            .borrow_mut()
            .insert(region_key.clone(), CdcAccumulator::from_rows(vec![]));
        self.region_streams.borrow_mut().insert(region_key, stream);
        Ok(())
    }

    pub async fn setup_all_blocks_watch(&self) -> Result<()> {
        let sql = "SELECT * FROM block";
        let stream = self
            .engine()
            .query_and_watch(sql.to_string(), HashMap::new(), None)
            .await?;
        *self.all_blocks.borrow_mut() = Some(CdcAccumulator::from_rows(vec![]));
        *self.all_blocks_stream.borrow_mut() = Some(stream);
        Ok(())
    }

    // =========================================================================
    // Navigation Operations
    // =========================================================================

    /// Navigate to focus on a specific block in a region.
    // navigate_focus / navigate_back / navigate_forward / navigate_home
    // were API-level shortcuts that bypassed the keyboard pipeline:
    // each called `execute_op("navigation", ...)` directly and then
    // mirrored `engine.ui_state().set_focus()` to fix up the
    // `maybe_mirror_navigation_focus` it had skipped.
    //
    // Removed by `frontends/tui/TODO.md` items A2–A4. The PBT now
    // drives navigation through the keyboard pipeline:
    //  - NavigateFocus → `driver.click_entity(doc, "left_sidebar")`
    //  - NavigateHome / Back / Forward → `send_leader_chord("h"|"b"|"f")`
    // See `crates/holon-integration-tests/src/pbt/sut.rs`.

    // =========================================================================
    // Watch Operations
    // =========================================================================

    /// Set up a CDC watch for a query in any supported language (prql/sql/gql).
    ///
    /// `&self` (not `&mut self`): the watch state is interior-mutable (see
    /// [`Self::active_watches`]), so this write path can be driven through the
    /// `&self` `SutWatchRegister` cap by the decomposed `SetupWatch` transition.
    /// The `.await` happens before any borrow is taken, so no guard crosses it.
    pub async fn setup_watch(
        &self,
        query_id: &str,
        source: &str,
        language: QueryLanguage,
    ) -> Result<()> {
        let stream = self
            .test_ctx()
            .query_and_watch(source.to_string(), language, HashMap::new())
            .await?;
        self.ui_model
            .borrow_mut()
            .insert(query_id.to_string(), CdcAccumulator::from_rows(vec![]));
        self.active_watches
            .borrow_mut()
            .insert(query_id.to_string(), stream);
        self.watch_queries
            .borrow_mut()
            .insert(query_id.to_string(), (source.to_string(), language));
        Ok(())
    }

    /// Remove a watch.
    pub fn remove_watch(&self, query_id: &str) {
        self.active_watches.borrow_mut().remove(query_id);
        self.watch_queries.borrow_mut().remove(query_id);
        self.ui_model.borrow_mut().remove(query_id);
    }

    // =========================================================================
    // View Operations
    // =========================================================================

    /// Switch the active view filter.
    pub fn switch_view(&self, view_name: &str) {
        *self.current_view.borrow_mut() = view_name.to_string();
    }

    // =========================================================================
    // Block CRUD Operations
    // =========================================================================

    /// Create a text block.
    pub async fn create_block(&self, id: &str, parent_id: &str, content: &str) -> Result<()> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert(
            "id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(id).to_string()),
        );
        params.insert(
            "parent_id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(parent_id).to_string()),
        );
        params.insert("content".into(), Value::String(content.to_string()));
        params.insert("content_type".into(), ContentType::Text.into());

        self.test_ctx().execute_op("block", "create", params).await
    }

    /// Create a source block with a specified language.
    pub async fn create_source_block(
        &self,
        id: &str,
        parent_id: &str,
        language: SourceLanguage,
        content: &str,
    ) -> Result<()> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert(
            "id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(id).to_string()),
        );
        params.insert(
            "parent_id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(parent_id).to_string()),
        );
        params.insert("content".into(), Value::String(content.to_string()));
        params.insert("content_type".into(), ContentType::Source.into());
        params.insert("source_language".into(), language.into());

        self.test_ctx().execute_op("block", "create", params).await
    }

    /// Update a block's content.
    pub async fn update_block_content(&self, id: &str, new_content: &str) -> Result<()> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert(
            "id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(id).to_string()),
        );
        params.insert("field".into(), Value::String("content".to_string()));
        params.insert("value".into(), Value::String(new_content.to_string()));

        self.test_ctx()
            .execute_op("block", "set_field", params)
            .await
    }

    /// Delete a block.
    pub async fn delete_block(&self, id: &str) -> Result<()> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert(
            "id".into(),
            // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
            Value::String(EntityUri::from_raw(id).to_string()),
        );

        self.test_ctx().execute_op("block", "delete", params).await
    }

    // =========================================================================
    // Polling / Waiting Helpers
    // =========================================================================

    /// Wait until a specific block exists in the database.
    pub async fn wait_for_block(&self, block_id: &str, timeout: std::time::Duration) -> bool {
        use crate::wait_until;

        // Callers address blocks by the bare id written in org files
        // (e.g. "block-1"); `block_raw` stores the schemed form
        // ("block:block-1"). Normalize at this boundary — idempotent for
        // already-schemed input (per ORG_SYNTAX: schemes live everywhere
        // outside org files).
        // ALLOW(entity_uri_from_raw): test-caller-supplied bare id entering helper; bare→schemed normalization at boundary
        let schemed = EntityUri::from_raw(block_id).to_string();
        let sql = format!("SELECT id FROM block_raw WHERE id = '{}'", schemed);
        let poll_interval = std::time::Duration::from_millis(50);

        wait_until(
            || async {
                self.test_ctx()
                    .query(sql.clone(), QueryLanguage::HolonSql, HashMap::new())
                    .await
                    .map(|rows| !rows.is_empty())
                    .unwrap_or(false)
            },
            timeout,
            poll_interval,
        )
        .await
    }

    /// Wait until the all-blocks CDC accumulator matches `expected_ids`
    /// (modulo seed blocks), then return the current set of non-page rows.
    ///
    /// Predicate (when seed_count is primed):
    ///   `acc.len() == expected_ids.len() + seed_count` AND
    ///   every id in `expected_ids` appears in `acc.state()`.
    ///
    /// The length term catches DELETE-pending states that subset alone
    /// misses (acc still holds the deleted block until the CDC delete
    /// event arrives). When seed_count hasn't been primed yet, falls back
    /// to subset-only — safe for the very first call (typically StartApp).
    ///
    /// Event-driven: awaits the next CDC batch (no polling). The 100 ms
    /// per-event ceiling surfaces a wedged stream as a timeout.
    pub async fn wait_for_blocks_synced(
        &self,
        expected_ids: &HashSet<EntityUri>,
        timeout: std::time::Duration,
    ) -> Vec<holon_api::StorageEntity> {
        self.wait_for_blocks_synced_with_content(expected_ids, &HashMap::new(), timeout)
            .await
    }

    /// [`Self::wait_for_blocks_synced`] plus per-id CONTENT convergence: each
    /// `(id, content)` in `expected_contents` must match the accumulator row's
    /// `content` column before the wait succeeds.
    ///
    /// Needed for same-id rewrites — an `index.org` layout swap under the
    /// fixed `:ID: root-layout` contract changes only the content of the
    /// existing layout block ids, so id-presence alone returns before the
    /// file watcher ingests the new text and the invariants see stale rows.
    pub async fn wait_for_blocks_synced_with_content(
        &self,
        expected_ids: &HashSet<EntityUri>,
        expected_contents: &HashMap<EntityUri, String>,
        timeout: std::time::Duration,
    ) -> Vec<holon_api::StorageEntity> {
        use tokio::time::{Duration, timeout as tokio_timeout};

        let start = std::time::Instant::now();
        let seed_count = self.seed_count.get();

        // Take the accumulator + stream out of their cells into locals so the
        // await-driven drain loop never holds a `RefCell` borrow across a
        // suspension point (the soundness rule for the `&self` flip). They are
        // restored before the method returns.
        let mut stream_opt = self.all_blocks_stream.borrow_mut().take();
        let mut acc_opt = self.all_blocks.borrow_mut().take();
        if let (Some(stream), Some(acc)) = (stream_opt.as_mut(), acc_opt.as_mut()) {
            loop {
                // Synthetic ref-side ids (`block::split-N`, `block:bulk-N-M`) are
                // placeholders the SUT replaces with real UUIDs — by construction
                // they NEVER appear in the CDC accumulator. Treat them as
                // count-only: the real block they stand for still counts toward
                // `length_ok`, but waiting for the synthetic id itself would
                // deliberately run to the timeout (it used to cost a flat 5s on
                // every PressKey-Enter under Turso).
                let subset_ok = expected_ids
                    .iter()
                    .filter(|id| !crate::pbt::is_synthetic_ref_id(id))
                    .all(|id| acc.state().contains_key(id.as_str()));
                let content_ok = expected_contents
                    .iter()
                    .filter(|(id, _)| !crate::pbt::is_synthetic_ref_id(id))
                    .all(|(id, content)| {
                        acc.state().get(id.as_str()).is_some_and(|row| {
                            row.get("content").and_then(|v| v.as_string()) == Some(content.as_str())
                        })
                    });
                let length_ok = match seed_count {
                    Some(seeds) => acc.state().len() == expected_ids.len() + seeds,
                    None => true,
                };
                if subset_ok && length_ok && content_ok {
                    break;
                }
                if start.elapsed() >= timeout {
                    break;
                }
                match tokio_timeout(Duration::from_millis(100), stream.next()).await {
                    Ok(Some(batch)) => {
                        for change in batch.inner.items {
                            acc.apply_change(rekey_change(change.change));
                        }
                    }
                    Ok(None) => break, // stream closed
                    Err(_) => {}       // event timeout — re-check predicate
                }
            }
        }
        // Restore the accumulator + stream into their cells.
        *self.all_blocks_stream.borrow_mut() = stream_opt;
        *self.all_blocks.borrow_mut() = acc_opt;

        // Return non-page rows (callers that inspect this expect filtering).
        //
        // Read from `block_raw` (writable base table), not the `block`
        // matview. After `wait_for_blocks_synced` succeeds (CDC accumulator
        // has all expected ids), an immediate SELECT against the matview
        // can still return fewer rows because the matview's IVM state is
        // mid-propagation — same class of race as the inv-viewmodel-root-matches-render-expr
        // `block_with_query_source.sql` issue
        // (devlog/2026-05-05-110311.md). `block_raw` is synchronously
        // written; the page-tag exclusion via `block_tags` is unaffected.
        let sql = "SELECT id FROM block_raw \
                   WHERE id NOT IN (SELECT block_id FROM block_tags WHERE tag = 'Page')"
            .to_string();
        self.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("wait_for_blocks_synced: block_raw query failed")
    }

    /// Capture the count of production seed blocks (no_parent sentinel,
    /// default sidebars, etc.) that aren't tracked by `ref_state`. Drains
    /// the all-blocks CDC stream until every expected id is present, then
    /// stores `acc.len() - expected_ids.len()` so future
    /// `wait_for_blocks_synced` calls can detect pending deletes via a
    /// length match. Call this once after StartApp's seeding settles.
    pub async fn prime_seed_count(
        &self,
        expected_ids: &HashSet<EntityUri>,
        timeout: std::time::Duration,
    ) {
        use tokio::time::{Duration, timeout as tokio_timeout};

        let start = std::time::Instant::now();

        // Take the accumulator + stream into locals so the await-driven drain
        // loop never holds a `RefCell` borrow across a suspension point; restore
        // them before returning.
        let mut stream_opt = self.all_blocks_stream.borrow_mut().take();
        let mut acc_opt = self.all_blocks.borrow_mut().take();
        if let (Some(stream), Some(acc)) = (stream_opt.as_mut(), acc_opt.as_mut()) {
            while !expected_ids
                .iter()
                .all(|id| acc.state().contains_key(id.as_str()))
            {
                if start.elapsed() >= timeout {
                    break;
                }
                match tokio_timeout(Duration::from_millis(100), stream.next()).await {
                    Ok(Some(batch)) => {
                        for change in batch.inner.items {
                            acc.apply_change(rekey_change(change.change));
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {}
                }
            }
            let extras = acc.state().len().saturating_sub(expected_ids.len());
            self.seed_count.set(Some(extras));
        }
        *self.all_blocks_stream.borrow_mut() = stream_opt;
        *self.all_blocks.borrow_mut() = acc_opt;
    }

    /// Simulate app restart by touching all org files to trigger re-parsing.
    /// This tests that re-parsing doesn't create orphan blocks.
    pub async fn simulate_restart(&self, expected_ids: &HashSet<EntityUri>) -> Result<()> {
        use std::time::Duration;

        let documents: Vec<(EntityUri, PathBuf)> = self
            .documents
            .borrow()
            .iter()
            .map(|(uri, path)| (uri.clone(), path.clone()))
            .collect();
        for (doc_uri, file_path) in &documents {
            eprintln!(
                "[simulate_restart] Re-triggering parse for: {} -> {}",
                doc_uri,
                file_path.display()
            );
            let content = self.org_fs.read_to_string(file_path).await.map_err(|e| {
                anyhow::anyhow!("simulate_restart: read {} failed: {e}", file_path.display())
            })?;
            // Add a space and remove it to ensure content is "different"
            let modified = format!("{} ", content);
            holon_filesystem::FileSystem::write(
                self.org_fs.as_ref(),
                file_path,
                modified.as_bytes(),
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "simulate_restart: touch-write {} failed: {e}",
                    file_path.display()
                )
            })?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            // Restore original content
            holon_filesystem::FileSystem::write(
                self.org_fs.as_ref(),
                file_path,
                content.as_bytes(),
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "simulate_restart: restore-write {} failed: {e}",
                    file_path.display()
                )
            })?;
        }

        // Wait for blocks to converge
        let timeout = Duration::from_millis(5000);
        let start = std::time::Instant::now();
        self.wait_for_blocks_synced(expected_ids, timeout).await;
        eprintln!(
            "[simulate_restart] Block count stabilized in {:?}",
            start.elapsed()
        );

        self.wait_for_org_files_stable(25, std::time::Duration::from_millis(5000))
            .await;

        Ok(())
    }

    // =========================================================================
    // External Mutation Helpers (for PBT and other tests)
    // =========================================================================

    /// Apply an external mutation by writing directly to org files.
    ///
    /// This simulates an external process (like Emacs) modifying the org file.
    /// The file watcher will detect the change and sync it to Loro.
    ///
    /// # Arguments
    /// * `expected_blocks` - All blocks that should exist after the mutation
    pub async fn apply_external_mutation(&self, expected_blocks: &[Block]) -> Result<()> {
        let grouped = holon_api::blocks_by_document(expected_blocks);
        let documents: Vec<(EntityUri, PathBuf)> = self
            .documents
            .borrow()
            .iter()
            .map(|(uri, path)| (uri.clone(), path.clone()))
            .collect();
        for (doc_uri, file_path) in &documents {
            let doc_blocks: Vec<&Block> = grouped
                .iter()
                .find(|(uri, _)| uri == doc_uri)
                .map(|(_, blocks)| blocks.iter().collect())
                .unwrap_or_default();

            let doc_block = expected_blocks
                .iter()
                .find(|b| b.id == *doc_uri && b.is_page());
            let org_content =
                crate::serialize_blocks_to_org_with_doc(&doc_blocks, doc_uri, doc_block);
            holon_filesystem::FileSystem::write(
                self.org_fs.as_ref(),
                file_path,
                org_content.as_bytes(),
            )
            .await?;
            tracing::trace!(
                "[apply_external_mutation] File written, org_content:\n{}",
                org_content
            );
        }

        tracing::trace!("[apply_external_mutation] File written, polling will wait for sync");
        Ok(())
    }

    /// Wait for the FileSyncController to be done re-rendering files.
    ///
    /// Fast path: if the controller's `OrgSyncIdleSignal` was wired through DI,
    /// wait until its loop has been idle for ~5 ms (event-driven). Then do a
    /// short mtime sanity check to catch the rare case where an EventBus
    /// publish hasn't yet reached the controller's subscriber channel.
    ///
    /// Fallback: if no signal is available (or the signal call times out),
    /// fall back to filesystem mtime polling for `stability_ms` quiescence.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_org_files_stable")]
    pub async fn wait_for_org_files_stable(&self, stability_ms: u64, timeout: std::time::Duration) {
        let start = std::time::Instant::now();

        // Phase 5: the former "strongest gate" (org consumer's EventBus ack
        // watermark) is gone — org no longer consumes block events, it
        // re-renders from the `LiveData<Block>` feed. Block settle is now
        // covered by `wait_for_live_data_mirrors` (the feed) at the call sites,
        // and the idle-signal gate below (which fires on the controller's
        // feed-driven `mark_progress`) plus the mtime fallback prove the
        // re-render itself drained. Polling a watermark the org consumer never
        // advances would only burn the full timeout.

        // Fast path: event-driven idle signal.
        if let Some(signal) = self.org_sync_idle.get() {
            // Use caller-supplied stability_ms instead of a hardcoded 5 ms —
            // the controller's event_bus delivery can have >5 ms latency
            // under PBT load (BulkExternalAdd flushes many events
            // sequentially), causing wait_quiescent to return "idle"
            // between events while a new event is still in-flight in the
            // mpsc channel. Symptoms: "Org file diverged from reference"
            // where actual blocks are missing entries that ref_state
            // expects (devlog/2026-05-05-110313.md). The stability_ms
            // parameter (callers pass 25 ms) was already plumbed through to
            // the mtime-polling fallback; threading it through here keeps
            // the two paths consistent.
            let signal_quiescence = std::time::Duration::from_millis(stability_ms);
            let signal_budget = std::time::Duration::from_millis(2000).min(timeout);
            let became_idle = signal
                .wait_quiescent(signal_quiescence, signal_budget)
                .await;
            if became_idle {
                // Controller is idle; verify mtime is also stable for a tiny
                // window to catch in-flight EventBus → subscriber latency.
                let remaining = timeout.saturating_sub(start.elapsed());
                self.poll_org_file_mtime_stable(
                    5,
                    remaining.min(std::time::Duration::from_millis(100)),
                )
                .await;
                return;
            }
            eprintln!(
                "[wait_for_org_files_stable] Idle signal did not quiesce within {:?}, falling back to mtime polling",
                signal_budget
            );
        }

        // Fallback: full mtime polling.
        let remaining = timeout.saturating_sub(start.elapsed());
        self.poll_org_file_mtime_stable(stability_ms, remaining)
            .await;
    }

    /// Poll until all org files stop changing (mtime stabilizes).
    ///
    /// Used as the fallback path of `wait_for_org_files_stable` and as a
    /// safety check after the event-driven idle signal fires.
    async fn poll_org_file_mtime_stable(&self, stability_ms: u64, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        let stability_duration = std::time::Duration::from_millis(stability_ms);
        let poll_interval = std::time::Duration::from_millis(5);

        let mut last_snapshot: HashMap<PathBuf, Option<std::time::SystemTime>> = HashMap::new();
        let mut stable_since = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                eprintln!(
                    "[poll_org_file_mtime_stable] Timed out after {:?} (stability={}ms)",
                    timeout, stability_ms
                );
                break;
            }

            let mut current_snapshot: HashMap<PathBuf, Option<std::time::SystemTime>> =
                HashMap::new();
            let file_paths: Vec<PathBuf> = self.documents.borrow().values().cloned().collect();
            for file_path in &file_paths {
                let mtime = self
                    .org_fs
                    .metadata(file_path)
                    .await
                    .ok() // ALLOW(ok): file may not exist
                    .map(|m| m.modified);
                current_snapshot.insert(file_path.clone(), mtime);
            }

            if current_snapshot == last_snapshot {
                if stable_since.elapsed() >= stability_duration {
                    break;
                }
            } else {
                stable_since = std::time::Instant::now();
                last_snapshot = current_snapshot;
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Poll until org files stop changing. Convenience wrapper with default parameters.
    pub async fn wait_for_write_window_expiry(&self) {
        self.wait_for_org_files_stable(25, std::time::Duration::from_millis(5000))
            .await;
    }

    /// Poll until org files stop changing after external processing.
    pub async fn wait_for_external_processing_expiry(&self) {
        self.wait_for_org_files_stable(25, std::time::Duration::from_millis(5000))
            .await;
    }
}

/// Wait until the `LoroSyncController`'s `last_synced` watermark matches the
/// global doc's current `oplog_frontiers()`, bounded by `timeout`. Shared by
/// [`TestEnvironment::wait_for_loro_quiescence`] and `LoroSut`'s peer-sync ops.
pub async fn wait_for_loro_quiescence_on(
    handle: &Arc<holon::sync::LoroSyncControllerHandle>,
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    timeout: std::time::Duration,
) {
    use tracing::field;
    let span = tracing::info_span!(
        "wait_for_loro_quiescence",
        timeout_ms = timeout.as_millis() as u64,
        attempts = field::Empty,
        timed_out = field::Empty,
    );
    let _enter = span.enter();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        let current = {
            let store = doc_store.read().await;
            store
                .get_global_doc()
                .await
                .expect("wait_for_loro_quiescence: get_global_doc failed")
                .doc()
                .oplog_frontiers()
        };
        if handle.last_synced_frontiers() == current {
            span.record("attempts", attempts);
            span.record("timed_out", false);
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            span.record("attempts", attempts);
            span.record("timed_out", true);
            eprintln!("[wait_for_loro_quiescence] timeout after {:?}", timeout);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// =============================================================================
// Backward Compatibility Aliases
// =============================================================================

/// Alias for backward compatibility
pub type TestContext = TestEnvironment;

/// Alias for backward compatibility
pub type TestContextBuilder = TestEnvironmentBuilder;

/// The CDC-quiescence "silence window" the PBT harness waits to confirm a
/// settled point (no new batch for this long ⇒ quiescent). Used by the three
/// test-only apply-path barriers — `CdcMirrors::wait_quiescent`, the
/// `wait_for_cdc_quiescent` call, and `assert_cdc_quiescent`'s catch-up grace.
///
/// Defaults to 25ms. The previous 50ms was the conservative starting floor;
/// 25ms validated green across a 192-transition / 8-case sweep with zero
/// `assert_cdc_quiescent` churn, settle-barrier-exhaustion, or content
/// divergence — the fail-loud no-churn assertion is the safety net that proves
/// the window is still wide enough. Override with `HOLON_PBT_QUIET_FLOOR_MS`
/// (e.g. `=50` to restore the old floor if CI ever flakes here).
///
/// Does NOT touch the production snapshot settle
/// (`TursoBlockQuerySource::DEFAULT_QUIET_FOR`, 50ms), which stays put.
pub(crate) fn pbt_quiet_floor() -> std::time::Duration {
    std::env::var("HOLON_PBT_QUIET_FLOOR_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| std::time::Duration::from_millis(25))
}

/// Compact one-line summary of a CDC change record. Used by inv-editable-text-has-draggable to
/// dump spurious leaked items without the noise of full Debug output.
/// Re-key a `Change<StorageEntity>` to the String-keyed `MapChange` shape that
/// `CdcAccumulator`/`ReactiveTable` (serde-facing `DataRow`) consume.
fn rekey_change(
    change: holon_api::Change<holon_api::StorageEntity>,
) -> holon_api::Change<holon_api::StorageEntity> {
    change
}

fn summarize_change(change: &holon_api::Change<holon_api::StorageEntity>) -> String {
    use holon_api::streaming::Change;
    match change {
        Change::Created { data, origin } => {
            format!("Created id={} origin={origin:?}", data_row_id(data))
        }
        Change::Updated { id, data, origin } => {
            format!(
                "Updated id={id} origin={origin:?} fields={:?}",
                data_row_field_names(data)
            )
        }
        Change::Deleted { id, origin } => {
            format!("Deleted id={id} origin={origin:?}")
        }
        Change::FieldsChanged {
            entity_id,
            fields,
            origin,
        } => {
            let pairs: Vec<String> = fields
                .iter()
                .map(|(name, old, new)| format!("{name}: {old:?} → {new:?}"))
                .collect();
            format!(
                "FieldsChanged id={entity_id} origin={origin:?} [{}]",
                pairs.join(", ")
            )
        }
    }
}

fn data_row_id(row: &holon_api::StorageEntity) -> String {
    row.get("id")
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "<no id>".to_string())
}

fn data_row_field_names(row: &holon_api::StorageEntity) -> Vec<&std::sync::Arc<str>> {
    row.keys().collect()
}
