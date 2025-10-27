//! Frontend session abstraction for Holon
//!
//! This crate provides a unified initialization protocol for all frontend consumers
//! (Flutter, TUI, integration tests, etc.). It ensures consistent setup including:
//!
//! - Module registration (OrgMode, Todoist)
//! - Waiting for background tasks to be ready
//! - Tracking startup errors
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_frontend::{FrontendConfig, FrontendSession};
//!
//! let config = FrontendConfig::new(UiInfo::permissive())
//!     .with_db_path("/path/to/db".into())
//!     .with_orgmode("/path/to/org/files".into());
//!
//! let session = FrontendSession::new(config).await?;
//!
//! // Use session methods directly - this guarantees initialization is complete
//! let (rx, cmd_tx) = session.watch_ui("root-layout".into(), None, true).await?;
//! ```

pub mod cdc;
pub mod cli;
pub mod command_menu;
pub mod geometry;
pub mod input;
pub mod input_trigger;
mod instance_config;
mod mcp_integrations;
pub mod memory_monitor;
pub mod navigation;
pub mod operation_matcher;
pub mod operations;
pub mod preferences;
mod render_context;
pub mod render_interpreter;
pub mod shadow_builders;
pub mod shadow_dom;
pub mod theme;
pub mod view_event_handler;
pub mod view_model;

pub use cdc::{spawn_ui_listener, AppState, CdcState};
use holon_api::EntityUri;
pub use input::{InputAction, Key, WidgetInput};
pub use instance_config::{InstanceConfig, UiSettings, WidgetState};
pub use mcp_integrations::McpIntegrationRegistry;
pub use navigation::{
    CollectionNavigator, CursorHint, CursorPlacement, ListNavigator, NavDirection, NavTarget,
    TableNavigator, TreeNavigator,
};
pub use preferences::{PrefKey, PrefSection, PrefType, PreferenceDef};
pub use render_context::{BlockWatchRegistry, RenderContext, RenderPipeline};
pub use shadow_builders::create_shadow_interpreter;
pub use shadow_dom::{KeyMap, ShadowDom};
pub use view_model::ViewModel;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use holon::api::BackendEngine;
use holon::di::create_backend_engine_with_extras;
use holon::sync::{
    CacheEventSubscriber, LoroConfig, LoroModule, PublishErrorTracker, TursoEventBus,
};
use holon_api::block::Block;
pub use holon_api::UiInfo;
use holon_orgmode::di::FileWatcherReadySignal;
use holon_todoist::di::TodoistServiceCollectionExt;

use holon::core::queryable_cache::QueryableCache;
use holon::di::DbHandleProvider;
use holon::sync::event_bus::EventBus;

// Re-export DiResolver for use in extra_resolve closures
pub use holon::di::DiResolver;

// Re-export types needed by consumers
pub use holon::api::backend_engine::QueryContext;
pub use holon::storage::turso::RowChangeStream;
pub use holon_api::{
    OperationDescriptor, ProviderAuthStatus, UiEvent, Value, WatcherCommand, WidgetSpec,
};

/// Marker type for the CacheEventSubscriber background wiring.
/// Resolving this from DI triggers the EventBus → QueryableCache subscription.
struct CacheEventSubscriberHandle;

/// Configuration for frontend session initialization
#[derive(Debug, Clone)]
pub struct FrontendConfig {
    /// Database file path (None = temporary file with random name)
    pub db_path: Option<PathBuf>,
    /// OrgMode root directory (None = disabled)
    pub orgmode_root: Option<PathBuf>,
    /// Enable Loro CRDT layer (default: false)
    pub loro_enabled: bool,
    /// Loro storage directory (default: orgmode_root/.loro or db_path dir/.loro)
    pub loro_storage_dir: Option<PathBuf>,
    /// Todoist API key (None = disabled)
    pub todoist_api_key: Option<String>,
    /// Enable Todoist with fake client (for testing)
    pub todoist_fake: bool,
    /// Whether to wait for file watcher readiness (default: true)
    pub wait_for_ready: bool,
    /// Frontend UI capability info for profile filtering (default: permissive)
    pub ui_info: UiInfo,
    /// Directory containing MCP integration YAML configs (None = disabled).
    /// Defaults to `~/.config/holon/integrations` when set to the default sentinel.
    pub mcp_integrations_dir: Option<PathBuf>,
}

impl FrontendConfig {
    pub fn new(ui_info: UiInfo) -> Self {
        Self {
            db_path: None,
            orgmode_root: None,
            loro_enabled: false,
            loro_storage_dir: None,
            todoist_api_key: None,
            todoist_fake: false,
            wait_for_ready: true,
            ui_info,
            mcp_integrations_dir: default_mcp_integrations_dir(),
        }
    }

    pub fn with_db_path(mut self, path: PathBuf) -> Self {
        self.db_path = Some(path);
        self
    }

    pub fn with_orgmode(mut self, root: PathBuf) -> Self {
        self.orgmode_root = Some(root);
        self
    }

    pub fn with_loro(mut self) -> Self {
        self.loro_enabled = true;
        self
    }

    pub fn with_loro_storage(mut self, dir: PathBuf) -> Self {
        self.loro_storage_dir = Some(dir);
        self
    }

    pub fn with_todoist(mut self, api_key: String) -> Self {
        self.todoist_api_key = Some(api_key);
        self
    }

    /// Enable Todoist with a fake in-memory client (for testing)
    ///
    /// This enables the same DI path as production (DDL, caches, streams),
    /// but uses a fake client instead of making real API calls.
    pub fn with_todoist_fake(mut self) -> Self {
        self.todoist_fake = true;
        self
    }

    pub fn without_wait(mut self) -> Self {
        self.wait_for_ready = false;
        self
    }

    pub fn with_mcp_integrations_dir(mut self, dir: PathBuf) -> Self {
        self.mcp_integrations_dir = Some(dir);
        self
    }

    pub fn without_mcp_integrations(mut self) -> Self {
        self.mcp_integrations_dir = None;
        self
    }
}

fn default_mcp_integrations_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/holon/integrations"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))
            .map(|c| PathBuf::from(c).join("holon/integrations"))
    }
}

/// Unified session for all frontend consumers (Flutter, TUI, tests)
///
/// Ensures consistent initialization:
/// 1. Registers configured modules
/// 2. Waits for background tasks to be ready
/// 3. Tracks startup errors
///
/// For tests that need additional services (e.g., LoroDocumentStore), use
/// `new_with_extras` which allows resolving additional services from DI.
pub struct FrontendSession<T = ()> {
    engine: Arc<BackendEngine>,
    error_tracker: PublishErrorTracker,
    ready_signal: Option<FileWatcherReadySignal>,
    /// Extra services resolved from DI (for tests)
    extras: T,
    /// Keeps the background memory monitor alive (logs RSS every 30s)
    _memory_monitor: Option<memory_monitor::MemoryMonitorHandle>,
    /// Persistent UI settings (theme, widget states). Protected by Mutex for interior mutability.
    config_state: Mutex<(InstanceConfig, PathBuf)>,
    /// Preference schema + theme registry, computed once at startup.
    preference_defs: Vec<preferences::PreferenceDef>,
    theme_registry: theme::ThemeRegistry,
}

impl FrontendSession<()> {
    /// Create a new frontend session with the given configuration
    ///
    /// This blocks until the system is ready (file watcher initialized, etc.)
    /// unless `wait_for_ready` is set to false in the config.
    pub async fn new(config: FrontendConfig) -> Result<Self> {
        Self::new_with_extras(config, |_| ()).await
    }

    /// Wrap an existing BackendEngine into a FrontendSession.
    ///
    /// Used by the PBT UI test: the PBT creates its own engine (via E2ESut),
    /// and this wraps it into a FrontendSession suitable for GLOBAL_SESSION
    /// so the Flutter app reuses the PBT's database.
    pub fn from_engine(engine: Arc<BackendEngine>) -> Self {
        let theme_registry = theme::ThemeRegistry::load(None);
        let preference_defs = preferences::define_preferences(&theme_registry);
        Self {
            engine,
            error_tracker: PublishErrorTracker::new(),
            ready_signal: None,
            extras: (),
            _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
            config_state: Mutex::new((InstanceConfig::default(), PathBuf::new())),
            preference_defs,
            theme_registry,
        }
    }
}

impl<T> FrontendSession<T> {
    /// Create a new frontend session with additional services resolved from DI
    ///
    /// This is useful for tests that need access to internal services like
    /// `LoroDocumentStore` for assertions.
    ///
    /// # Example
    /// ```rust,ignore
    /// let session = FrontendSession::new_with_extras(config, |provider| {
    ///     let store = DiResolver::get_required::<LoroDocumentStore>(provider);
    ///     Arc::new(RwLock::new((*store).clone()))
    /// }).await?;
    /// ```
    pub async fn new_with_extras<F>(config: FrontendConfig, extra_resolve: F) -> Result<Self>
    where
        F: FnOnce(&ferrous_di::ServiceProvider) -> T,
    {
        #[cfg(not(target_arch = "wasm32"))]
        let db_path = config.db_path.unwrap_or_else(|| {
            std::env::temp_dir().join(format!("holon-{}.db", uuid::Uuid::new_v4()))
        });

        #[cfg(target_arch = "wasm32")]
        let db_path = config.db_path.unwrap_or_else(|| PathBuf::from(":memory:"));

        let config_dir = db_path.parent().unwrap_or(&db_path).to_path_buf();
        let instance_config = InstanceConfig::load(&config_dir);
        let instance_config_for_session = instance_config.clone();
        let post_org_write_hook = instance_config.hooks.post_org_write.clone();

        let orgmode_root = config.orgmode_root.clone();
        let loro_enabled = config.loro_enabled;
        let loro_storage_dir = config.loro_storage_dir.clone();
        let todoist_key = config.todoist_api_key.clone();
        let todoist_fake = config.todoist_fake;
        let ui_info = config.ui_info.clone();
        let mcp_integrations_dir = config.mcp_integrations_dir.clone();

        let (engine, resolved) = create_backend_engine_with_extras(
            db_path.clone(),
            move |services| {
                use ferrous_di::ServiceCollectionModuleExt;

                services.add_singleton(ui_info);

                // Register shared event infrastructure when any data pipeline is active.
                // TursoEventBus, QueryableCache<Block>, and CacheEventSubscriber are needed
                // by both Loro and OrgMode. Register them here so either can work standalone.
                if loro_enabled || orgmode_root.is_some() {
                    services.add_singleton_factory::<QueryableCache<Block>, _>(|r| {
                        holon::di::create_queryable_cache(r)
                    });

                    services.add_singleton_factory::<TursoEventBus, _>(|resolver| {
                        let db_handle_provider = ferrous_di::Resolver::get_required_trait::<
                            dyn holon::di::DbHandleProvider,
                        >(resolver);
                        let event_bus = TursoEventBus::new(db_handle_provider.handle());
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                event_bus
                                    .init_schema()
                                    .await
                                    .expect("Failed to initialize EventBus schema");
                            })
                        });
                        event_bus
                    });

                    services.add_singleton(PublishErrorTracker::new());

                    // Wire CacheEventSubscriber: EventBus → QueryableCache (blocks, dirs, files)
                    // Registered as a marker type so DI triggers the wiring.
                    services.add_singleton_factory::<CacheEventSubscriberHandle, _>(
                        |resolver| {
                            let block_cache =
                                ferrous_di::Resolver::get_required::<QueryableCache<Block>>(
                                    resolver,
                                );
                            let event_bus =
                                ferrous_di::Resolver::get_required::<TursoEventBus>(resolver);
                            let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();

                            // Optionally resolve dir/file caches (registered by OrgModeModule)
                            let dir_cache = ferrous_di::Resolver::get::<
                                holon::core::queryable_cache::QueryableCache<
                                    holon_filesystem::directory::Directory,
                                >,
                            >(resolver)
                            .ok();
                            let file_cache = ferrous_di::Resolver::get::<
                                holon::core::queryable_cache::QueryableCache<
                                    holon_filesystem::File,
                                >,
                            >(resolver)
                            .ok();

                            tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current().block_on(async {
                                    let subscriber = CacheEventSubscriber::new(block_cache);
                                    if let Err(e) = subscriber.start(event_bus_arc.clone()).await {
                                        tracing::error!(
                                            "[FrontendSession] Failed to start CacheEventSubscriber: {}",
                                            e
                                        );
                                    }

                                    // Subscribe dir/file caches if available
                                    if let Some(dc) = dir_cache {
                                        if let Err(e) = CacheEventSubscriber::subscribe_entity(
                                            holon::sync::event_bus::AggregateType::Directory, dc, event_bus_arc.clone(),
                                        ).await {
                                            tracing::error!(
                                                "[FrontendSession] Failed to subscribe directory cache: {}",
                                                e
                                            );
                                        }
                                    }
                                    if let Some(fc) = file_cache {
                                        if let Err(e) = CacheEventSubscriber::subscribe_entity(
                                            holon::sync::event_bus::AggregateType::File, fc, event_bus_arc,
                                        ).await {
                                            tracing::error!(
                                                "[FrontendSession] Failed to subscribe file cache: {}",
                                                e
                                            );
                                        }
                                    }
                                });
                            });

                            CacheEventSubscriberHandle
                        },
                    );
                }

                // Register Loro module if enabled (must be before OrgMode so OrgMode can detect it)
                let resolved_loro_dir = if loro_enabled {
                    let loro_dir = loro_storage_dir
                        .clone()
                        .or_else(|| orgmode_root.as_ref().map(|r| r.join(".loro")))
                        .unwrap_or_else(|| db_path.parent().unwrap_or(&db_path).join(".loro"));
                    services.add_singleton(LoroConfig::new(loro_dir.clone()));
                    services
                        .add_module_mut(LoroModule)
                        .map_err(|e| anyhow::anyhow!("Failed to register LoroModule: {}", e))?;
                    Some(loro_dir)
                } else {
                    None
                };

                if let Some(root) = orgmode_root {
                    if let Some(loro_dir) = resolved_loro_dir {
                        use holon_orgmode::di::{OrgModeConfig, OrgModeModule};
                        let mut org_config = OrgModeConfig::with_loro_storage(root, loro_dir);
                        org_config.post_org_write_hook = post_org_write_hook.clone();
                        services.add_singleton(org_config);
                        services.add_module_mut(OrgModeModule).map_err(|e| {
                            anyhow::anyhow!("Failed to register OrgModeModule: {}", e)
                        })?;
                    } else {
                        use holon_orgmode::di::{OrgModeConfig, OrgModeModule};
                        let mut org_config = OrgModeConfig::new(root);
                        org_config.post_org_write_hook = post_org_write_hook.clone();
                        services.add_singleton(org_config);
                        services.add_module_mut(OrgModeModule)?;
                    }
                }
                if let Some(key) = todoist_key {
                    let mut todoist_config = instance_config.todoist.clone();
                    todoist_config.api_key = Some(key);
                    services.add_todoist(todoist_config)?;
                } else if todoist_fake {
                    #[cfg(not(target_arch = "wasm32"))]
                    services.add_todoist_fake()?;
                }

                // Register MCP integrations from config directory
                if let Some(ref dir) = mcp_integrations_dir {
                    let module = mcp_integrations::McpIntegrationsModule::from_dir(dir);
                    services.add_module_mut(module).map_err(|e| {
                        anyhow::anyhow!("Failed to register McpIntegrationsModule: {}", e)
                    })?;
                }

                Ok(())
            },
            |provider| {
                // Trigger lazy DI resolution of background wiring handles
                let _ = DiResolver::get::<CacheEventSubscriberHandle>(provider);
                let _ = DiResolver::get::<holon::sync::LoroEventAdapterHandle>(provider);
                // Eagerly build MCP integrations (connects to servers, runs initial sync)
                let _ = DiResolver::get::<McpIntegrationRegistry>(provider);

                let error_tracker: PublishErrorTracker =
                    DiResolver::get::<PublishErrorTracker>(provider)
                        .map(|t| (*t).clone())
                        .unwrap_or_else(|_| PublishErrorTracker::new());
                // FileWatcherReadySignal is only registered when OrgMode is enabled
                let ready_signal: Option<FileWatcherReadySignal> =
                    DiResolver::get::<FileWatcherReadySignal>(provider)
                        .ok()
                        .map(|s| (*s).clone());

                // Resolve DbHandle for transition_to_ready()
                let db_handle_provider: Option<std::sync::Arc<dyn DbHandleProvider>> =
                    DiResolver::get_trait::<dyn DbHandleProvider>(provider).ok();

                // Resolve caller's extra services
                let extras = extra_resolve(provider);

                (
                    error_tracker,
                    ready_signal,
                    db_handle_provider,
                    extras,
                )
            },
        )
        .await?;

        let (error_tracker, ready_signal, db_handle_provider, extras) = resolved;

        // CRITICAL: Signal that DDL phase is complete AFTER BackendEngine is fully resolved.
        // This ensures ALL DDL (including OperationLogStore, NavigationProvider, etc.)
        // is complete before OrgMode background tasks start publishing events.
        // The DatabaseActor serializes all database operations, eliminating race conditions.
        if let Some(provider) = db_handle_provider {
            let handle = provider.handle();
            if let Err(e) = handle.transition_to_ready().await {
                tracing::warn!(
                    "[FrontendSession] Failed to transition actor to ready: {}",
                    e
                );
            }
        }

        // Seed default layout BEFORE waiting for orgmode — this gives the UI
        // something to render immediately. Once orgmode finishes syncing real
        // org files, CDC fires and UiWatcher re-renders with the real layout.
        // On subsequent startups where real data exists, this is a no-op.
        Self::seed_default_layout(&engine).await?;

        // Wait for orgmode readiness if configured. With watch_ui's CDC-driven
        // architecture, the UI updates progressively as data arrives — so this
        // is mainly needed by tests that assert on final state.
        if config.wait_for_ready {
            if let Some(ref signal) = ready_signal {
                signal.wait_ready().await;
            }
        }

        let theme_registry = theme::ThemeRegistry::load(None);
        let preference_defs = preferences::define_preferences(&theme_registry);

        Ok(Self {
            engine,
            error_tracker,
            ready_signal,
            extras,
            _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
            config_state: Mutex::new((instance_config_for_session, config_dir)),
            preference_defs,
            theme_registry,
        })
    }

    /// Get the extra services resolved from DI
    pub fn extras(&self) -> &T {
        &self.extras
    }

    /// Get the backend engine
    pub fn engine(&self) -> &Arc<BackendEngine> {
        &self.engine
    }

    /// Resolve the entity profile for a data row.
    ///
    /// Returns the matched profile (with render expression and operations),
    /// or `None` only when no entity type could be inferred from the row.
    /// Operations are always injected by ProfileResolver from the single
    /// source of truth (OperationDispatcher).
    pub fn resolve_row_profile(
        &self,
        row: &holon_api::widget_spec::DataRow,
    ) -> Option<holon::entity_profile::RowProfile> {
        let profile = self.engine.profile_resolver().resolve(row);
        if profile.name == "fallback" && profile.operations.is_empty() {
            None
        } else {
            Some(profile.as_ref().clone())
        }
    }

    /// Read current UI settings.
    pub fn ui_settings(&self) -> UiSettings {
        self.config_state.lock().unwrap().0.ui.clone()
    }

    /// Mutate UI settings and persist to disk.
    pub fn update_ui_settings(&self, f: impl FnOnce(&mut UiSettings)) {
        let mut guard = self.config_state.lock().unwrap();
        f(&mut guard.0.ui);
        guard.0.save(&guard.1);
    }

    /// Look up widget state by block ID. Returns default (open=true) if not found.
    pub fn widget_state(&self, block_id: &str) -> WidgetState {
        self.config_state
            .lock()
            .unwrap()
            .0
            .ui
            .widgets
            .get(block_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Toggle a widget's open state and persist.
    pub fn set_widget_open(&self, block_id: &str, open: bool) {
        self.update_ui_settings(|s| {
            s.widgets.entry(block_id.to_string()).or_default().open = open;
        });
    }

    // =========================================================================
    // Preferences API
    // =========================================================================

    /// Get the preference schema definitions.
    pub fn preference_defs(&self) -> &[preferences::PreferenceDef] {
        &self.preference_defs
    }

    /// Get the theme registry.
    pub fn theme_registry(&self) -> &theme::ThemeRegistry {
        &self.theme_registry
    }

    /// Read a preference value. Returns the stored value or the definition's default.
    pub fn get_preference(&self, key: &preferences::PrefKey) -> toml::Value {
        let guard = self.config_state.lock().unwrap();
        guard.0.get_preference(key).cloned().unwrap_or_else(|| {
            self.preference_defs
                .iter()
                .find(|d| d.key == *key)
                .map(|d| d.default.clone())
                .unwrap_or(toml::Value::String(String::new()))
        })
    }

    /// Set a preference value and persist to disk.
    pub fn set_preference(&self, key: &preferences::PrefKey, value: toml::Value) {
        let mut guard = self.config_state.lock().unwrap();
        guard.0.set_preference(key, value);
        guard.0.save(&guard.1);
    }

    /// Generate the render data for the preferences UI.
    ///
    /// Returns a `RenderExpr` tree and data rows. Frontends interpret the
    /// expression with their existing `RenderInterpreter` / builder registry.
    pub fn preferences_render_data(
        &self,
    ) -> (
        holon_api::render_types::RenderExpr,
        Vec<HashMap<String, Value>>,
    ) {
        let current = self.config_state.lock().unwrap().0.preferences.clone();
        let expr = preferences::preferences_render_expr(&self.preference_defs);
        let rows = preferences::preferences_to_rows(&self.preference_defs, &current);
        (expr, rows)
    }

    /// Check if there were any startup errors (DDL/sync races)
    pub fn has_startup_errors(&self) -> bool {
        self.error_tracker.has_errors()
    }

    /// Get the number of startup errors
    pub fn startup_error_count(&self) -> usize {
        self.error_tracker.errors()
    }

    /// Get the error tracker for detailed monitoring
    pub fn error_tracker(&self) -> &PublishErrorTracker {
        &self.error_tracker
    }

    /// Check if the file watcher is ready (useful for tests)
    pub fn is_ready(&self) -> bool {
        self.ready_signal.as_ref().map_or(true, |s| s.is_ready())
    }

    // =========================================================================
    // Default Layout Seeding
    // =========================================================================

    fn default_doc_uri() -> holon_api::EntityUri {
        holon_api::EntityUri::doc("__default__")
    }

    /// Seed a default layout into the database if no real layout exists.
    ///
    /// On fresh installations (no org directory), the app needs a root layout
    /// to render the 3-column UI. This parses the bundled `index.org` and
    /// creates blocks under a well-known `doc:__default__` document.
    ///
    /// When a real `index.org` is later synced, the next startup detects the
    /// real layout and cleans up the seeded blocks.
    ///
    /// Uses raw SQL via db_handle because OperationProviders may not be
    /// registered (e.g. TUI without orgmode). This is a bootstrap operation
    /// that doesn't need events, undo, or observers.
    async fn seed_default_layout(engine: &BackendEngine) -> Result<()> {
        let db = engine.db_handle();

        // Check if the fixed root layout block already exists
        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID;
        let rows = db
            .query(
                &format!("SELECT id FROM block WHERE id = '{root_id}'"),
                HashMap::new(),
            )
            .await?;
        if !rows.is_empty() {
            Self::cleanup_seeded_blocks(engine).await?;
            return Ok(());
        }

        let content = include_str!("../../../assets/default/index.org");
        let path = Path::new("index.org");
        let root = Path::new("");
        let default_doc_uri = Self::default_doc_uri();
        let parse_result = holon_orgmode::parse_org_file(path, content, &default_doc_uri, 0, root)?;

        // The parser generates doc URI from the file path (doc:index.org).
        // We need to rewrite top-level block parent_ids to our well-known doc URI.
        let file_doc_uri = parse_result.document.id.clone();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_millis() as i64;

        // Create the seeded document via raw SQL
        db.execute(
            &format!(
                "INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at) \
                 VALUES ('{}', 'doc:__root__', '__default__', 'a0', '{{}}', {}, {})",
                default_doc_uri, now, now
            ),
            vec![],
        )
        .await?;

        for block in &parse_result.blocks {
            let parent_id = if block.parent_id == file_doc_uri {
                default_doc_uri.clone()
            } else {
                block.parent_id.clone()
            };
            let params = holon_orgmode::build_block_params(block, &parent_id, &default_doc_uri);

            // Partition params into known SQL columns and extra properties,
            // mirroring SqlOperationProvider::partition_params behavior.
            let known_columns: std::collections::HashSet<&str> = [
                "id",
                "parent_id",
                "document_id",
                "depth",
                "sort_key",
                "content",
                "content_type",
                "source_language",
                "source_name",
                "properties",
                "collapsed",
                "completed",
                "block_type",
                "created_at",
                "updated_at",
                "_change_origin",
            ]
            .into_iter()
            .collect();

            let mut columns = Vec::new();
            let mut values = Vec::new();
            let mut extra_props = HashMap::new();

            for (key, value) in &params {
                if key == "properties" {
                    // Merge existing properties JSON into extra_props
                    if let Some(s) = value.as_string() {
                        if let Ok(map) =
                            serde_json::from_str::<HashMap<String, serde_json::Value>>(s)
                        {
                            for (k, v) in map {
                                extra_props.insert(k, serde_json::Value::from(v));
                            }
                        }
                    }
                } else if known_columns.contains(key.as_str()) {
                    columns.push(format!("\"{}\"", key));
                    values.push(holon::storage::sql_utils::value_to_sql_literal(value));
                } else {
                    let json_val = match value {
                        Value::String(s) => serde_json::Value::String(s.clone()),
                        Value::Integer(i) => serde_json::json!(i),
                        Value::Float(f) => serde_json::json!(f),
                        Value::Boolean(b) => serde_json::json!(b),
                        _ => serde_json::Value::String(format!("{:?}", value)),
                    };
                    extra_props.insert(key.clone(), json_val);
                }
            }

            if !extra_props.is_empty() {
                let props_json =
                    serde_json::to_string(&extra_props).unwrap_or_else(|_| "{}".to_string());
                columns.push("\"properties\"".to_string());
                values.push(format!("'{}'", props_json.replace('\'', "''")));
            }

            let sql = format!(
                "INSERT OR REPLACE INTO block ({}) VALUES ({})",
                columns.join(", "),
                values.join(", ")
            );
            db.execute(&sql, vec![]).await?;
        }

        tracing::info!(
            "[FrontendSession] Seeded default layout ({} blocks)",
            parse_result.blocks.len()
        );
        Ok(())
    }

    /// Remove seeded default blocks when a real layout is available.
    async fn cleanup_seeded_blocks(engine: &BackendEngine) -> Result<()> {
        let db = engine.db_handle();

        // Check if a non-seeded layout exists (any block with this ID under a real document)
        let rows = db
            .query(
                &format!(
                    "SELECT document_id FROM block WHERE id = '{}' AND document_id != '{}'",
                    holon_api::ROOT_LAYOUT_BLOCK_ID,
                    Self::default_doc_uri()
                ),
                HashMap::new(),
            )
            .await
            .unwrap_or_default();

        if rows.is_empty() {
            return Ok(());
        }

        // A real layout exists — delete all seeded blocks and the seeded document
        db.execute(
            &format!(
                "DELETE FROM block WHERE document_id = '{}'",
                Self::default_doc_uri()
            ),
            vec![],
        )
        .await?;

        db.execute(
            &format!(
                "DELETE FROM document WHERE id = '{}'",
                Self::default_doc_uri()
            ),
            vec![],
        )
        .await?;

        tracing::info!("[FrontendSession] Cleaned up seeded default layout");
        Ok(())
    }

    // =========================================================================
    // Query Methods - These can only be called after initialization completes
    // =========================================================================

    /// Get the initial widget for the application root
    /// Watch a block's UI with automatic error recovery and structural hot-swap.
    ///
    /// Returns a long-lived stream of `UiEvent`s (Structure + Data) and a command
    /// channel for variant switching. Unlike `render_block`, errors become
    /// `UiEvent::Structure` events with error WidgetSpecs — the stream stays open
    /// and recovers when the underlying block is fixed.
    pub async fn watch_ui(
        &self,
        block_id: &EntityUri,
        is_root: bool,
    ) -> Result<holon_api::WatchHandle> {
        holon::api::watch_ui(Arc::clone(&self.engine), block_id.clone(), is_root).await
    }

    /// Execute a PRQL query and set up CDC streaming
    ///
    /// This is the main query method for reactive UI updates.
    /// Returns a `WidgetSpec` with initial data and a stream for CDC updates.
    ///
    /// # Arguments
    /// * `prql` - The PRQL query to execute
    /// * `params` - Query parameters
    /// * `context` - Optional query context (for `from children` resolution)
    pub async fn query_and_watch(
        &self,
        prql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<(WidgetSpec, RowChangeStream)> {
        self.engine.query_and_watch(prql, params, context).await
    }

    /// Execute an operation on an entity
    ///
    /// Operations mutate the database. UI updates happen via CDC streams.
    /// This follows unidirectional data flow: Action → Model → View
    ///
    /// # Arguments
    /// * `entity_name` - The entity to operate on (e.g., "blocks", "documents")
    /// * `op_name` - The operation name (e.g., "create", "delete", "set_field")
    /// * `params` - Operation parameters
    pub async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: HashMap<String, Value>,
    ) -> Result<Option<Value>> {
        self.engine
            .execute_operation(entity_name, op_name, params)
            .await
    }

    /// Get available operations for an entity
    ///
    /// Returns a list of operation descriptors available for the given entity_name.
    /// Use "*" as entity_name to get wildcard operations.
    pub async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.engine.available_operations(entity_name).await
    }

    /// Check if an operation is available for an entity
    pub async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        self.engine.has_operation(entity_name, op_name).await
    }

    /// Undo the last operation
    ///
    /// Returns true if an operation was undone, false if the undo stack is empty.
    pub async fn undo(&self) -> Result<bool> {
        self.engine.undo().await
    }

    /// Redo the last undone operation
    ///
    /// Returns true if an operation was redone, false if the redo stack is empty.
    pub async fn redo(&self) -> Result<bool> {
        self.engine.redo().await
    }

    /// Check if undo is available
    pub async fn can_undo(&self) -> bool {
        self.engine.can_undo().await
    }

    /// Check if redo is available
    pub async fn can_redo(&self) -> bool {
        self.engine.can_redo().await
    }

    /// Look up a block's path from the blocks_with_paths materialized view
    ///
    /// Returns the hierarchical path for a block (e.g., "/parent/block_id").
    /// This path is used for descendants queries via path prefix matching.
    pub async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        self.engine.blocks().lookup_block_path(block_id).await
    }

    /// Execute a raw SQL query
    ///
    /// This is a lower-level method for direct SQL access.
    /// Prefer `query_and_watch` for reactive queries.
    pub async fn execute_query(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<HashMap<String, Value>>> {
        self.engine.execute_query(sql, params, context).await
    }
}
