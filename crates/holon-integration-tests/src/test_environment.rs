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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::{assign_reference_sequences, wait_for_file_condition};
use holon_api::reactive::CdcAccumulator;

use holon::api::backend_engine::QueryContext;
use holon::api::{BackendEngine, RowChangeStream};
use holon::sync::LoroDocumentStore;
use holon::sync::event_bus::PublishErrorTracker;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::{ContentType, QueryLanguage, Region, RenderExpr, SourceLanguage, Value};
use holon_frontend::{FrontendSession, HolonConfig, SessionConfig};

/// Types of corruption for stale .loro files (for testing recovery)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoroCorruptionType {
    /// Empty file (0 bytes)
    Empty,
    /// File with partial/truncated Loro header
    Truncated,
    /// File with invalid magic bytes
    InvalidHeader,
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

    /// The running application (None before start_app())
    session: Option<Arc<FrontendSession>>,

    /// Loro doc store, resolved from DI (None when Loro is disabled)
    loro_doc_store: Option<Arc<RwLock<LoroDocumentStore>>>,

    /// Loro sync controller handle, resolved from DI (None when Loro is disabled).
    /// Used by `wait_for_loro_quiescence` to poll until the controller has
    /// caught up with the current Loro state.
    loro_sync_handle: Option<Arc<holon::sync::LoroSyncControllerHandle>>,

    /// Reactive engine, resolved from DI (same instance as GPUI uses).
    /// Provides BuilderServices, keybinding registry, operation dispatch.
    pub reactive_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,

    /// Idle signal for the OrgSyncController loop. When present, lets
    /// `wait_for_org_files_stable` skip filesystem polling on the hot path.
    org_sync_idle: Option<Arc<holon_orgmode::OrgSyncIdleSignal>>,

    /// EventBus handle for watermark-based consumer-catchup waits. Same
    /// instance the LoroSyncController / OrgSyncController /
    /// CacheEventSubscriber subscribe to, so polling
    /// `consumer_position(c)` against `watermark()` tells us when each
    /// downstream consumer has caught up to the latest published events.
    pub event_bus: Option<Arc<holon::sync::TursoEventBus>>,

    /// The E2ETestContext for operations (wraps BackendEngine) - only valid after start_app()
    ctx: Option<E2ETestContext>,

    /// Created documents (doc_uri -> file path)
    pub documents: HashMap<EntityUri, PathBuf>,

    /// Active CDC watches (query_id -> stream)
    pub active_watches: HashMap<String, RowChangeStream>,

    /// Watch query metadata for fallback re-query (query_id -> (source, language))
    pub watch_queries: HashMap<String, (String, QueryLanguage)>,

    /// UI model built from CDC events (query_id -> accumulator)
    pub ui_model: HashMap<String, CdcAccumulator<HashMap<String, Value>>>,

    /// Current view filter
    pub current_view: String,

    /// Region CDC streams from AppFrame (region_id -> stream)
    pub region_streams: HashMap<String, RowChangeStream>,

    /// Region data built from CDC events (region_id -> accumulator)
    pub region_data: HashMap<String, CdcAccumulator<HashMap<String, Value>>>,

    /// All-blocks CDC watch for invariant #1 (uses production CdcAccumulator)
    pub all_blocks: Option<CdcAccumulator<HashMap<String, Value>>>,

    /// All-blocks CDC stream
    all_blocks_stream: Option<RowChangeStream>,

    /// Whether to enable Todoist fake mode (adds concurrent DDL during startup)
    enable_todoist: bool,

    /// Whether to enable Loro CRDT layer (default: true for backward compat)
    enable_loro: bool,
}

impl std::fmt::Debug for TestEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestEnvironment")
            .field("documents", &self.documents)
            .field("temp_dir", &self.temp_dir.path())
            .field("is_running", &self.session.is_some())
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
    /// Enable Todoist with fake client (for testing DDL race conditions)
    enable_todoist_fake: bool,
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
            enable_todoist_fake: false,
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

    /// Enable Todoist with a fake in-memory client.
    ///
    /// This enables the same DI path as production (DDL for `todoist_tasks` and
    /// `todoist_projects` tables, same caches, streams, and event adapters),
    /// but uses a fake client instead of making real API calls.
    ///
    /// This is critical for testing the DDL race condition where Todoist tables
    /// are created concurrently with OrgMode sync events.
    pub fn with_todoist_fake(mut self) -> Self {
        self.enable_todoist_fake = true;
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

        // Write pre-populated org files BEFORE engine initialization
        // This is the key to reproducing the Flutter bug
        let mut documents = HashMap::new();
        for (filename, content) in &self.org_files {
            let file_path = temp_dir.path().join(filename);
            tokio::fs::write(&file_path, content)
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
        if self.enable_todoist_fake {
            session_config = session_config.with_todoist_fake();
        }

        let (session, (doc_store, reactive_engine, sync_handle, idle_signal, event_bus)) =
            FrontendSession::new_from_config_with_di(
                holon_config,
                session_config,
                config_dir,
                std::collections::HashSet::new(),
                |injector| {
                    use holon_frontend::reactive::{
                        BuilderServicesSlot, RenderInterpreterInjectorExt,
                    };
                    let slot = injector.resolve::<BuilderServicesSlot>();
                    injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                        slot.0.clone(),
                    ));
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
                    let event_bus = injector.try_resolve::<holon::sync::TursoEventBus>().ok();
                    (doc_store, engine, sync_handle, idle_signal, event_bus)
                },
            )
            .await?;

        // Tests need deterministic state — wait for CDC event propagation
        if settle_delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(settle_delay_ms)).await;
        }

        let ctx = E2ETestContext::from_engine(session.engine().clone());

        let _startup_errors = session.error_tracker().errors();

        Ok(TestEnvironment {
            temp_dir,
            runtime,
            session: Some(session),
            loro_doc_store: doc_store,
            loro_sync_handle: sync_handle,
            reactive_engine: Some(reactive_engine),
            org_sync_idle: idle_signal,
            event_bus,
            ctx: Some(ctx),
            documents,
            active_watches: HashMap::new(),
            watch_queries: HashMap::new(),
            ui_model: HashMap::new(),
            current_view: "all".to_string(),
            region_streams: HashMap::new(),
            region_data: HashMap::new(),
            all_blocks: None,
            all_blocks_stream: None,
            enable_todoist: self.enable_todoist_fake,
            enable_loro,
        })
    }
}

impl Default for TestEnvironmentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestEnvironment {
    /// Create a new test environment (app not started yet).
    ///
    /// Use this for pre-startup testing scenarios. Call `start_app()` to start the application.
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        let temp_dir =
            TempDir::new().map_err(|e| anyhow::anyhow!("Failed to create temp dir: {}", e))?;

        Ok(Self {
            temp_dir,
            runtime,
            session: None,
            loro_doc_store: None,
            loro_sync_handle: None,
            reactive_engine: None,
            org_sync_idle: None,
            event_bus: None,
            ctx: None,
            documents: HashMap::new(),
            active_watches: HashMap::new(),
            watch_queries: HashMap::new(),
            ui_model: HashMap::new(),
            current_view: "all".to_string(),
            region_streams: HashMap::new(),
            region_data: HashMap::new(),
            all_blocks: None,
            all_blocks_stream: None,
            enable_todoist: false,
            enable_loro: true,
        })
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
    pub async fn write_org_file(&mut self, filename: &str, content: &str) -> Result<PathBuf> {
        let file_path = self.temp_dir.path().join(filename);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create parent directories: {}", e))?;
        }

        tokio::fs::write(&file_path, content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write org file: {}", e))?;

        let doc_uri = EntityUri::file(filename);
        self.documents.insert(doc_uri, file_path.clone());

        // Small delay to ensure file watcher detects the change (only if app is running)
        if self.session.is_some() {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        Ok(file_path)
    }

    /// Write a stale/corrupted .loro file to the temp directory.
    ///
    /// This simulates scenarios where a .loro file exists from a previous run
    /// but is corrupted or empty. The system should detect this and recover.
    ///
    /// Can only be called BEFORE `start_app()`.
    pub async fn write_stale_loro_file(
        &mut self,
        filename: &str,
        corruption_type: LoroCorruptionType,
    ) -> Result<PathBuf> {
        assert!(
            self.session.is_none(),
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

    /// Enable Todoist fake mode for the next start_app() call.
    ///
    /// When enabled, start_app() will include Todoist with a fake client,
    /// which adds concurrent DDL (CREATE TABLE todoist_tasks, todoist_projects)
    /// during startup. This increases the race window and matches production DI path.
    pub fn set_enable_todoist(&mut self, enable: bool) {
        self.enable_todoist = enable;
    }

    /// Set whether to enable Loro CRDT layer for the next start_app() call.
    pub fn set_enable_loro(&mut self, enable: bool) {
        self.enable_loro = enable;
    }

    /// Whether Loro is enabled for this environment.
    pub fn loro_enabled(&self) -> bool {
        self.enable_loro
    }

    /// Start the application.
    ///
    /// This triggers sync of any pre-existing files and may race with DDL.
    ///
    /// # Arguments
    /// * `wait_for_ready` - If true, wait for file watcher to be ready before returning
    pub async fn start_app(&mut self, wait_for_ready: bool) -> Result<()> {
        assert!(self.session.is_none(), "App already started");
        holon_frontend::shadow_builders::register_render_dsl_widget_names();

        let holon_config = HolonConfig {
            db_path: Some(self.temp_dir.path().join("test.db")),
            orgmode: holon_frontend::config::OrgmodeConfig {
                root_directory: Some(self.temp_dir.path().to_path_buf()),
            },
            loro: holon_frontend::config::LoroPreferences {
                enabled: if self.enable_loro { Some(true) } else { None },
                ..Default::default()
            },
            ..Default::default()
        };
        let config_dir = self.temp_dir.path().to_path_buf();
        let mut session_config = SessionConfig::new(holon_api::UiInfo::permissive());
        if !wait_for_ready {
            session_config = session_config.without_wait();
        }
        if self.enable_todoist {
            session_config = session_config.with_todoist_fake();
        }

        let enable_loro = self.enable_loro;
        let (session, (doc_store, reactive_engine, sync_handle, idle_signal, event_bus)) =
            FrontendSession::new_from_config_with_di(
                holon_config,
                session_config,
                config_dir,
                std::collections::HashSet::new(),
                |injector| {
                    use holon_frontend::reactive::{
                        BuilderServicesSlot, RenderInterpreterInjectorExt,
                    };
                    let slot = injector.resolve::<BuilderServicesSlot>();
                    injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                        slot.0.clone(),
                    ));
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
                    let event_bus = injector.try_resolve::<holon::sync::TursoEventBus>().ok();
                    (doc_store, engine, sync_handle, idle_signal, event_bus)
                },
            )
            .await?;

        let ctx = E2ETestContext::from_engine(session.engine().clone());

        self.session = Some(session);
        self.loro_doc_store = doc_store;
        self.loro_sync_handle = sync_handle;
        self.reactive_engine = Some(reactive_engine);
        self.org_sync_idle = idle_signal;
        self.event_bus = event_bus;
        self.ctx = Some(ctx);

        Ok(())
    }

    /// Check if app is running
    pub fn is_running(&self) -> bool {
        self.session.is_some()
    }

    /// Get the running session (panics if not started)
    pub fn session(&self) -> &FrontendSession {
        self.session
            .as_ref()
            .expect("App not started - call start_app() first")
    }

    /// Get the running session as an Arc (panics if not started)
    pub fn session_arc(&self) -> Arc<FrontendSession> {
        Arc::clone(
            self.session
                .as_ref()
                .expect("App not started - call start_app() first"),
        )
    }

    /// Get the E2ETestContext (panics if not started)
    ///
    /// Use this for direct access to the test context operations.
    pub fn test_ctx(&self) -> &E2ETestContext {
        self.ctx
            .as_ref()
            .expect("App not started - call start_app() first")
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

    /// Get the underlying engine (requires running app)
    pub fn engine(&self) -> &Arc<BackendEngine> {
        self.session().engine()
    }

    /// Get the doc store (requires running app with Loro enabled).
    /// Returns None when Loro is disabled.
    pub fn doc_store(&self) -> Option<&Arc<RwLock<LoroDocumentStore>>> {
        self.loro_doc_store.as_ref()
    }

    /// Number of errors logged by the `LoroSyncController` since startup.
    /// Returns 0 when Loro is disabled (handle is None).
    pub fn loro_sync_error_count(&self) -> usize {
        self.loro_sync_handle
            .as_ref()
            .map(|h| h.error_count())
            .unwrap_or(0)
    }

    /// Wait until every named EventBus consumer has caught up to the
    /// current published watermark, or until `timeout` elapses.
    ///
    /// Replaces a fixed `tokio::time::sleep(100ms)` that used to give
    /// `LoroSyncController` / `OrgSyncController` /
    /// `CacheEventSubscriber` "time to drain" the events emitted by a
    /// just-finished transition. The watermark is `MAX(events.created_at)`
    /// at call time; per-consumer position is `MAX(created_at) WHERE
    /// processed_by_<consumer> = 1`. When the matview chain settles
    /// inside a few ms we no longer pay the full 100 ms.
    ///
    /// No-op when no EventBus is wired (in-memory configs).
    pub async fn wait_for_consumers(&self, consumers: &[&str], timeout: std::time::Duration) {
        use holon::sync::event_bus::EventBus;
        let Some(bus) = &self.event_bus else { return };
        let bus: &dyn EventBus = bus.as_ref();
        let target = match bus.watermark().await {
            Ok(t) if t > 0 => t,
            _ => return,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let mut delay = std::time::Duration::from_millis(1);
        loop {
            let mut all_caught_up = true;
            for c in consumers {
                let pos = bus.consumer_position(c).await.unwrap_or(0);
                if pos < target {
                    all_caught_up = false;
                    break;
                }
            }
            if all_caught_up {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                eprintln!(
                    "[wait_for_consumers] timeout: consumers {consumers:?} did not reach watermark {target} within {timeout:?}",
                );
                return;
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(std::time::Duration::from_millis(100));
        }
    }

    /// Wait for the `LoroSyncController` to reach quiescence — i.e., its
    /// `last_synced` watermark matches the current `oplog_frontiers()`.
    /// No-op when Loro is disabled.
    pub async fn wait_for_loro_quiescence(&self, timeout: std::time::Duration) {
        let (Some(handle), Some(doc_store)) =
            (self.loro_sync_handle.as_ref(), self.loro_doc_store.as_ref())
        else {
            return;
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let current = {
                let store = doc_store.read().await;
                match store.get_global_doc().await {
                    Ok(collab) => {
                        let doc = collab.doc();
                        doc.oplog_frontiers()
                    }
                    Err(_) => return,
                }
            };
            if handle.last_synced_frontiers() == current {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                eprintln!("[wait_for_loro_quiescence] timeout after {:?}", timeout,);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Create an org file in the temp directory (requires running app).
    ///
    /// When Loro is enabled, also loads the file into the LoroDocumentStore.
    /// When Loro is disabled, just writes the file and tracks it.
    pub async fn create_document(&mut self, file_name: &str) -> Result<EntityUri> {
        let file_path = self.temp_dir.path().join(file_name);
        tokio::fs::write(&file_path, "")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create org file: {}", e))?;

        if let Some(doc_store) = self.doc_store() {
            let mut store = doc_store.write().await;
            store
                .get_or_load(&file_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to load org file: {}", e))?;
        }

        // Wait for OrgSyncController to create the document entity with a UUID.
        let doc_name = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name);
        let sql = format!(
            "SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' \
             AND substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) = '{}'",
            doc_name
        );
        let timeout = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();
        let doc_uri = loop {
            if let Ok(rows) = self.query_sql(&sql).await
                && let Some(row) = rows.first()
                && let Some(id) = row.get("id").and_then(|v| v.as_string())
            {
                break EntityUri::parse(id)?;
            }
            assert!(
                start.elapsed() < timeout,
                "Timeout waiting for document entity for '{}'",
                file_name
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        self.documents.insert(doc_uri.clone(), file_path);

        Ok(doc_uri)
    }

    /// Execute an operation on the backend
    pub async fn execute_operation(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        self.test_ctx().execute_op(entity, op, params).await
    }

    /// Query the backend
    pub async fn query(
        &self,
        source: &str,
        language: QueryLanguage,
    ) -> Result<Vec<HashMap<String, Value>>> {
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
            "SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' \
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

        let sql = format!(
            "SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' \
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
    pub async fn query_sql(&self, sql: &str) -> Result<Vec<HashMap<String, Value>>> {
        self.query(sql, QueryLanguage::HolonSql).await
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
        self.temp_dir.path().join(file_name)
    }

    /// Get the temp directory path
    pub fn temp_path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    /// Get path to a document by doc_uri
    pub fn get_document_path(&self, doc_uri: &EntityUri) -> Option<&PathBuf> {
        self.documents.get(doc_uri)
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
                "SELECT id, content, content_type, source_language, parent_id FROM block WHERE parent_id = 'block:root-layout' OR id = 'block:root-layout'",
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
        let sql = session.engine().compile_to_sql(source, language)?;
        let block_path = session.lookup_block_path(context_block_id).await?;
        let context = QueryContext::for_block_with_path(context_block_id, None, block_path);
        let rows = session
            .execute_query(sql, HashMap::new(), Some(context))
            .await?;
        Ok(rows)
    }

    /// Simulate what the Flutter UI does when rendering a query source block.
    ///
    /// When the UI encounters `render_entity this` for a source block,
    /// it should execute the query with the source block's PARENT as context.
    /// This is because `from children` in that query should get children of
    /// the heading (parent), not children of the source block itself.
    /// Uses FrontendSession directly to ensure identical code path with Flutter.
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
        let session = self.session();

        // First, get the source block to find its content, language, and parent
        let blocks = session
            .execute_query(
                "SELECT parent_id, content, source_language FROM block WHERE id = $id".to_string(),
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

    /// Drain CDC events from all active watches and update ui_model.
    #[tracing::instrument(skip(self), name = "pbt.drain_cdc_events")]
    pub async fn drain_cdc_events(&mut self) {
        use futures::FutureExt;

        // Drain CDC events without blocking. We sleep briefly up front to give
        // producer tasks real wall time to run (CDC forwarders, Loro sync),
        // then `now_or_never` every subsequent poll so an empty channel exits
        // immediately. 5 ms was picked after pure `yield_now` caused Loro
        // quiescence races in the cross-executor PBT variant — the short
        // sleep gives other executors a chance to make progress.
        //
        // Correctness gate: inv1 (SQL = ref), inv3 (UI model = ref), and inv8
        // (region focus roots) all start failing if the producer hasn't
        // actually delivered events by the time we poll. If they flake, bump
        // the sleep.
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

        for (query_id, stream) in &mut self.active_watches {
            let mut event_count = 0;
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        event_count += batch.inner.items.len();
                        if let Some(ui_data) = self.ui_model.get_mut(query_id) {
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
                                ui_data.apply_change(change.change.clone());
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

        if let (Some(stream), Some(acc)) = (&mut self.all_blocks_stream, &mut self.all_blocks) {
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        for change in batch.inner.items {
                            acc.apply_change(change.change);
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

        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

        for (region_id, stream) in &mut self.region_streams {
            let mut event_count = 0;
            loop {
                match stream.next().now_or_never() {
                    Some(Some(batch)) => {
                        event_count += batch.inner.items.len();
                        if let Some(region_data) = self.region_data.get_mut(region_id) {
                            for change in &batch.inner.items {
                                region_data.apply_change(change.change.clone());
                            }
                        }
                    }
                    _ => break,
                }
            }
            if event_count > 0 {
                eprintln!(
                    "[drain_region_cdc] region '{}': drained {} CDC events",
                    region_id, event_count
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

        if self.active_watches.is_empty()
            && self.region_streams.is_empty()
            && self.all_blocks_stream.is_none()
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
            .as_ref()
            .map(|c| c.engine().db_handle().cdc_emitted_watermark())
            .unwrap_or(0);

        // Bounded poll loop. We exit early as soon as every active stream
        // has seen `seq >= target_seq` (or has nothing pending). Any batch
        // whose `seq > target_seq` is a churn event and recorded for the
        // failure assertion. The previous implementation slept a fixed
        // 50 ms — when the cascade settled in <1 ms (the common case) we
        // wasted 49 ms per transition.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(50);
        let mut spurious: Vec<(String, usize)> = Vec::new();
        // For each spurious source, keep a compact one-line summary of every
        // change record so a failure dump shows what actually leaked, not
        // just the count. inv16 is the panic path, so the cost of the
        // extra Strings only ever matters when the test is failing.
        let mut spurious_dump: Vec<(String, u64, String)> = Vec::new();
        let mut watch_seen: HashMap<String, u64> = HashMap::new();
        let mut region_seen: HashMap<String, u64> = HashMap::new();
        let mut all_blocks_seen: u64 = 0;

        loop {
            let mut still_pending = false;

            for (query_id, stream) in &mut self.active_watches {
                let mut count = 0usize;
                while let Some(Some(batch)) = stream.next().now_or_never() {
                    let batch_seq = batch.metadata.seq;
                    let known_seq = watch_seen.entry(query_id.clone()).or_insert(0);
                    *known_seq = (*known_seq).max(batch_seq);
                    if batch_seq > target_seq {
                        count += batch.inner.items.len();
                        for change in &batch.inner.items {
                            spurious_dump.push((
                                format!("watch:{query_id}"),
                                batch_seq,
                                summarize_change(&change.change),
                            ));
                        }
                    }
                    if let Some(ui_data) = self.ui_model.get_mut(query_id) {
                        for change in &batch.inner.items {
                            ui_data.apply_change(change.change.clone());
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

            for (region_id, stream) in &mut self.region_streams {
                let mut count = 0usize;
                while let Some(Some(batch)) = stream.next().now_or_never() {
                    let batch_seq = batch.metadata.seq;
                    let known_seq = region_seen.entry(region_id.clone()).or_insert(0);
                    *known_seq = (*known_seq).max(batch_seq);
                    if batch_seq > target_seq {
                        count += batch.inner.items.len();
                        for change in &batch.inner.items {
                            spurious_dump.push((
                                format!("region:{region_id}"),
                                batch_seq,
                                summarize_change(&change.change),
                            ));
                        }
                    }
                    if let Some(region_data) = self.region_data.get_mut(region_id) {
                        for change in &batch.inner.items {
                            region_data.apply_change(change.change.clone());
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

            if let (Some(stream), Some(acc)) = (&mut self.all_blocks_stream, &mut self.all_blocks) {
                let mut count = 0usize;
                while let Some(Some(batch)) = stream.next().now_or_never() {
                    let batch_seq = batch.metadata.seq;
                    all_blocks_seen = all_blocks_seen.max(batch_seq);
                    if batch_seq > target_seq {
                        count += batch.inner.items.len();
                        for change in &batch.inner.items {
                            spurious_dump.push((
                                "all_blocks".to_string(),
                                batch_seq,
                                summarize_change(&change.change),
                            ));
                        }
                    }
                    for change in batch.inner.items {
                        acc.apply_change(change.change);
                    }
                }
                if count > 0 {
                    spurious.push(("all_blocks".to_string(), count));
                }
                if all_blocks_seen < target_seq {
                    still_pending = true;
                }
            }

            if !still_pending {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }

        if !spurious.is_empty() {
            eprintln!(
                "[inv16] CDC not quiescent — spurious events after seq watermark {target_seq}: {:?}",
                spurious,
            );
            // Dump every leaked change so the panic log is enough to
            // identify which writes are firing CDC, without needing MCP
            // attachment or sqlite inspection.
            eprintln!("[inv16] spurious change records (source, seq, change):");
            for (source, seq, summary) in &spurious_dump {
                eprintln!("    [{source} seq={seq}] {summary}");
            }
            crate::debug_pause::pause_on_fail(&format!(
                "inv16 CDC quiescence violation — spurious events after seq watermark \
                 {target_seq}: {:?}",
                spurious,
            ));
        }

        assert!(
            spurious.is_empty(),
            "[inv16] CDC not quiescent after settlement — spurious events: {:?}. \
             This indicates the backend is churning (emitting add/remove cycles \
             for unchanged data).",
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
    /// recognizes custom keywords — matching how production OrgSyncController
    /// stores keywords on the Document entity.
    pub async fn parse_org_file_blocks(&self, todo_header: Option<&str>) -> Result<Vec<Block>> {
        use holon_orgmode::parser::parse_org_file;

        let mut all_blocks = Vec::new();
        let root = self.temp_dir.path();

        for file_path in self.documents.values() {
            let raw = tokio::fs::read_to_string(file_path).await?;
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
    pub async fn setup_region_watch(&mut self, region: Region) -> Result<()> {
        let sql = format!(
            "SELECT fr.root_id AS id, b.content, b.parent_id \
             FROM focus_roots fr \
             JOIN block b ON b.id = fr.root_id \
             WHERE fr.region = '{}'",
            region.as_str()
        );
        let stream = self
            .session()
            .engine()
            .query_and_watch(sql, HashMap::new(), None)
            .await?;

        let region_key = region.as_str().to_string();
        self.region_data
            .insert(region_key.clone(), CdcAccumulator::from_rows(vec![]));
        self.region_streams.insert(region_key, stream);
        Ok(())
    }

    pub async fn setup_all_blocks_watch(&mut self) -> Result<()> {
        let sql = "SELECT * FROM block";
        let stream = self
            .session()
            .engine()
            .query_and_watch(sql.to_string(), HashMap::new(), None)
            .await?;
        self.all_blocks = Some(CdcAccumulator::from_rows(vec![]));
        self.all_blocks_stream = Some(stream);
        Ok(())
    }

    // =========================================================================
    // Navigation Operations
    // =========================================================================

    /// Navigate to focus on a specific block in a region.
    pub async fn navigate_focus(&mut self, region: Region, block_id: &EntityUri) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        params.insert("block_id".to_string(), block_id.clone().into());
        self.test_ctx()
            .execute_op("navigation", "focus", params)
            .await?;
        self.drain_region_cdc_events().await;
        // Mirror `UiState.focused_block` from the navigation target so
        // `focus_chain()` providers see the update. Production wires
        // this via `ReactiveEngine::dispatch_intent` but the PBT uses
        // `execute_op` directly, bypassing that path.
        if let Some(engine) = &self.reactive_engine {
            engine.ui_state().set_focus(Some(block_id.clone()));
        }
        Ok(())
    }

    /// Navigate back in history for a region.
    pub async fn navigate_back(&mut self, region: Region) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        self.test_ctx()
            .execute_op("navigation", "go_back", params)
            .await?;
        self.drain_region_cdc_events().await;
        Ok(())
    }

    /// Navigate forward in history for a region.
    pub async fn navigate_forward(&mut self, region: Region) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        self.test_ctx()
            .execute_op("navigation", "go_forward", params)
            .await?;
        self.drain_region_cdc_events().await;
        Ok(())
    }

    /// Navigate to home (root view) for a region.
    pub async fn navigate_home(&mut self, region: Region) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        self.test_ctx()
            .execute_op("navigation", "go_home", params)
            .await?;
        self.drain_region_cdc_events().await;
        if let Some(engine) = &self.reactive_engine {
            engine.ui_state().set_focus(None);
        }
        Ok(())
    }

    // =========================================================================
    // Watch Operations
    // =========================================================================

    /// Set up a CDC watch for a query in any supported language (prql/sql/gql).
    pub async fn setup_watch(
        &mut self,
        query_id: &str,
        source: &str,
        language: QueryLanguage,
    ) -> Result<()> {
        let stream = self
            .test_ctx()
            .query_and_watch(source.to_string(), language, HashMap::new())
            .await?;
        self.ui_model
            .insert(query_id.to_string(), CdcAccumulator::from_rows(vec![]));
        self.active_watches.insert(query_id.to_string(), stream);
        self.watch_queries
            .insert(query_id.to_string(), (source.to_string(), language));
        Ok(())
    }

    /// Remove a watch.
    pub fn remove_watch(&mut self, query_id: &str) {
        self.active_watches.remove(query_id);
        self.watch_queries.remove(query_id);
        self.ui_model.remove(query_id);
    }

    // =========================================================================
    // View Operations
    // =========================================================================

    /// Switch the active view filter.
    pub fn switch_view(&mut self, view_name: &str) {
        self.current_view = view_name.to_string();
    }

    // =========================================================================
    // Block CRUD Operations
    // =========================================================================

    /// Create a text block.
    pub async fn create_block(&self, id: &str, parent_id: &str, content: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        params.insert(
            "parent_id".to_string(),
            Value::String(parent_id.to_string()),
        );
        params.insert("content".to_string(), Value::String(content.to_string()));
        params.insert("content_type".to_string(), ContentType::Text.into());

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
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        params.insert(
            "parent_id".to_string(),
            Value::String(parent_id.to_string()),
        );
        params.insert("content".to_string(), Value::String(content.to_string()));
        params.insert("content_type".to_string(), ContentType::Source.into());
        params.insert("source_language".to_string(), language.into());

        self.test_ctx().execute_op("block", "create", params).await
    }

    /// Update a block's content.
    pub async fn update_block_content(&self, id: &str, new_content: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert("value".to_string(), Value::String(new_content.to_string()));

        self.test_ctx()
            .execute_op("block", "set_field", params)
            .await
    }

    /// Delete a block.
    pub async fn delete_block(&self, id: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));

        self.test_ctx().execute_op("block", "delete", params).await
    }

    // =========================================================================
    // Polling / Waiting Helpers
    // =========================================================================

    /// Wait until a specific block exists in the database.
    pub async fn wait_for_block(&self, block_id: &str, timeout: std::time::Duration) -> bool {
        use crate::wait_until;

        let sql = format!("SELECT id FROM block WHERE id = '{}'", block_id);
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

    /// Wait until expected block count is reached in the database.
    /// Returns the actual rows if condition met, or last result if timed out.
    ///
    /// Uses the all-blocks CDC accumulator for zero-SQL polling: drains the CDC
    /// stream until the accumulator has `expected_count` non-document blocks,
    /// then verifies with a single SQL query to return the actual rows.
    pub async fn wait_for_block_count(
        &mut self,
        expected_count: usize,
        timeout: std::time::Duration,
    ) -> Vec<HashMap<String, Value>> {
        use tokio::time::{Duration, timeout as tokio_timeout};

        let start = std::time::Instant::now();

        // CDC-based waiting: drain all-blocks stream until count matches.
        // Each CDC event is free (no SQL query — matview push).
        if let (Some(stream), Some(acc)) = (&mut self.all_blocks_stream, &mut self.all_blocks) {
            while start.elapsed() < timeout {
                let non_doc_count = acc.state().len();
                if non_doc_count == expected_count {
                    break;
                }
                // Drain next batch of CDC events (with short timeout per batch)
                match tokio_timeout(Duration::from_millis(100), stream.next()).await {
                    Ok(Some(batch)) => {
                        for change in batch.inner.items {
                            acc.apply_change(change.change);
                        }
                    }
                    Ok(None) => break, // stream closed
                    Err(_) => {}       // timeout — loop will re-check count
                }
            }
        }

        // Final verification via single SQL query (1 read instead of N polls).
        // Non-page blocks are those without a "Page" tag in block_tags.
        let sql = "SELECT id FROM block \
                   WHERE id NOT IN (SELECT block_id FROM block_tags WHERE tag = 'Page')"
            .to_string();
        self.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .unwrap_or_default()
    }

    /// Simulate app restart by touching all org files to trigger re-parsing.
    /// This tests that re-parsing doesn't create orphan blocks.
    pub async fn simulate_restart(&mut self, expected_block_count: usize) -> Result<()> {
        use std::time::Duration;

        for (doc_uri, file_path) in &self.documents {
            eprintln!(
                "[simulate_restart] Re-triggering parse for: {} -> {}",
                doc_uri,
                file_path.display()
            );
            if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
                // Add a space and remove it to ensure content is "different"
                let modified = format!("{} ", content);
                let _ = tokio::fs::write(&file_path, &modified).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                // Restore original content
                let _ = tokio::fs::write(&file_path, &content).await;
            }
        }

        // Wait for block count to stabilize
        let timeout = Duration::from_millis(5000);
        let start = std::time::Instant::now();
        let _ = self
            .wait_for_block_count(expected_block_count, timeout)
            .await;
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
        for (doc_uri, file_path) in &self.documents {
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
            tokio::fs::write(file_path, &org_content).await?;
            eprintln!(
                "[apply_external_mutation] File written, org_content:\n{}",
                org_content
            );
        }

        eprintln!("[apply_external_mutation] File written, polling will wait for sync");
        Ok(())
    }

    /// Wait for org files to sync to the expected block count.
    ///
    /// This waits for each document's org file to contain the expected number of blocks
    /// based on the reference blocks provided.
    ///
    /// # Arguments
    /// * `expected_blocks` - Reference blocks to count expected blocks per document
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// `true` if all files synced within timeout, `false` otherwise
    #[tracing::instrument(skip(self, expected_blocks), name = "pbt.wait_for_org_file_sync")]
    pub async fn wait_for_org_file_sync(
        &self,
        expected_blocks: &[Block],
        timeout: std::time::Duration,
    ) -> bool {
        let start = std::time::Instant::now();

        let grouped = holon_api::blocks_by_document(expected_blocks);
        for (doc_uri, file_path) in &self.documents {
            let expected_in_doc: usize = grouped
                .iter()
                .find(|(uri, _)| uri == doc_uri)
                .map(|(_, blocks)| blocks.len())
                .unwrap_or(0);

            let mut doc_blocks: Vec<Block> = grouped
                .iter()
                .find(|(uri, _)| uri == doc_uri)
                .map(|(_, blocks)| blocks.clone())
                .unwrap_or_default();
            assign_reference_sequences(&mut doc_blocks);
            // Hash the body only — always render WITHOUT the `#+TODO:` header.
            // Production may or may not carry the keyword set on each doc block
            // (it only does after the StartApp push for the default doc, and after
            // file parsing for pre-seeded regular files), so a header-sensitive
            // hash produces 5 s timeouts in cases where the body is already
            // correct. inv2's block-equivalence assertion is the real correctness
            // gate — this hash is just a sync-completion heuristic.
            let expected_org = holon_orgmode::org_renderer::OrgRenderer::render_entitys(
                &doc_blocks,
                file_path,
                doc_uri,
            );
            let expected_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                expected_org.trim().hash(&mut hasher);
                hasher.finish()
            };

            // Strip a leading `#+TODO: ...` header from actual content before
            // hashing — same rationale as above.
            fn strip_todo_header(content: &str) -> &str {
                let trimmed = content.trim_start();
                if trimmed.starts_with("#+TODO:") {
                    trimmed
                        .find('\n')
                        .map(|i| trimmed[i + 1..].trim_start())
                        .unwrap_or(trimmed)
                } else {
                    trimmed
                }
            }

            let remaining = timeout.saturating_sub(start.elapsed());
            let condition_met = wait_for_file_condition(
                file_path,
                |content| {
                    let text_count = content.matches(":ID:").count();
                    let src_count = content.to_lowercase().matches("#+begin_src").count();
                    let actual_count = text_count + src_count;
                    if actual_count != expected_in_doc {
                        return false;
                    }
                    // Check content hash matches expected rendered output
                    let actual_hash = {
                        use std::hash::{Hash, Hasher};
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        strip_todo_header(content).trim().hash(&mut hasher);
                        hasher.finish()
                    };
                    actual_hash == expected_hash
                },
                remaining,
            )
            .await;

            if condition_met {
                eprintln!(
                    "[wait_for_org_file_sync] Org file {:?} synced ({} blocks) in {:?}",
                    file_path,
                    expected_in_doc,
                    start.elapsed()
                );
            } else {
                // Debug: show what differs
                if let Ok(actual_content) = std::fs::read_to_string(file_path) {
                    let actual_count = actual_content.matches(":ID:").count()
                        + actual_content.to_lowercase().matches("#+begin_src").count();
                    if actual_count != expected_in_doc {
                        eprintln!(
                            "[wait_for_org_file_sync] WARNING: Org file {:?} block count mismatch: actual={} expected={} after {:?}",
                            file_path,
                            actual_count,
                            expected_in_doc,
                            start.elapsed()
                        );
                    } else {
                        eprintln!(
                            "[wait_for_org_file_sync] WARNING: Org file {:?} hash mismatch after {:?}\n  EXPECTED:\n{}\n  ACTUAL:\n{}",
                            file_path,
                            start.elapsed(),
                            expected_org.lines().take(15).collect::<Vec<_>>().join("\n"),
                            actual_content
                                .lines()
                                .take(15)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                    }
                } else {
                    eprintln!(
                        "[wait_for_org_file_sync] WARNING: Org file {:?} not synced after {:?}",
                        file_path,
                        start.elapsed()
                    );
                }
                return false;
            }
        }
        true
    }

    /// Wait for the OrgSyncController to be done re-rendering files.
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

        // Fast path: event-driven idle signal.
        if let Some(signal) = &self.org_sync_idle {
            let signal_quiescence = std::time::Duration::from_millis(5);
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
            for file_path in self.documents.values() {
                let mtime = tokio::fs::metadata(file_path)
                    .await
                    .ok() // ALLOW(ok): file may not exist
                    .and_then(|m| m.modified().ok()); // ALLOW(ok): file may not exist
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

// =============================================================================
// Backward Compatibility Aliases
// =============================================================================

/// Alias for backward compatibility
pub type TestContext = TestEnvironment;

/// Alias for backward compatibility
pub type TestContextBuilder = TestEnvironmentBuilder;

/// Compact one-line summary of a CDC change record. Used by inv16 to
/// dump spurious leaked items without the noise of full Debug output.
fn summarize_change(change: &holon_api::streaming::MapChange) -> String {
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

fn data_row_id(row: &holon_api::widget_spec::DataRow) -> String {
    row.get("id")
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "<no id>".to_string())
}

fn data_row_field_names(row: &holon_api::widget_spec::DataRow) -> Vec<&String> {
    row.keys().collect()
}

/// True when a CDC row's `tags` value contains the literal `"Page"` tag.
///
/// The `tags` column is `#[jsonb]` and CDC events deliver it in any of these
/// shapes: `Value::Array(["Page", ...])`, `Value::Json("[\"Page\",...]")`,
/// `Value::String("[\"Page\",...]")`, `Value::Null`, or absent. We check all
/// shapes uniformly so the page filter doesn't silently misclassify by shape.
fn row_tags_contain_page(row: &holon_api::widget_spec::DataRow) -> bool {
    use holon_api::Value;
    let Some(value) = row.get("tags") else {
        return false;
    };
    match value {
        Value::Array(arr) => arr
            .iter()
            .any(|v| matches!(v, Value::String(s) if s == "Page")),
        Value::Json(s) | Value::String(s) => {
            if s.is_empty() {
                false
            } else {
                serde_json::from_str::<Vec<String>>(s)
                    .map(|tags| tags.iter().any(|t| t == "Page"))
                    .unwrap_or_else(|e| {
                        panic!(
                            "[row_tags_contain_page] tags column contained invalid JSON {:?}: {}",
                            s, e
                        )
                    })
            }
        }
        Value::Null => false,
        other => panic!(
            "[row_tags_contain_page] unexpected tags value shape: {:?}",
            other
        ),
    }
}
