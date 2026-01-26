//! Frontend session abstraction for Holon
//!
//! Uses `premortem` for layered config (Defaults → TOML → CLI/env) and `clap` for
//! CLI parsing. Configuration is defined once in [`config::HolonConfig`] and automatically
//! gets CLI + env var + TOML file support.
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_frontend::{FrontendSession, cli};
//!
//! let (config, session_cfg, config_dir, locked) =
//!     cli::build_session(widgets)?;
//! let session = FrontendSession::new_from_config(
//!     config, session_cfg, config_dir, locked,
//! ).await?;
//! ```

pub mod cdc;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod collection_layout;
pub mod lane_filtered_provider;

/// A default org file bundled with the app, seeded on first launch.
pub struct DefaultAsset {
    pub filename: &'static str,
    pub content: &'static str,
    /// Fixed document block ID. Enables org content to reference its own document
    /// (e.g., `parent_id == 'block:journals'`). `None` means random UUID.
    pub fixed_doc_id: Option<&'static str>,
}

/// Default assets seeded when the org root has no `.org` files.
/// Production seeding and PBT reference model both consume this list.
pub const DEFAULT_ASSETS: &[DefaultAsset] = &[DefaultAsset {
    filename: "Journals.org",
    content: include_str!("../../../assets/default/Journals.org"),
    fixed_doc_id: Some("block:journals"),
}];
pub mod command_provider;
pub mod config;
pub mod editable_text_provider;
pub mod editor_controller;
pub mod focus_path;
pub mod frontend_module;
pub mod geometry;
pub use geometry::vms_button_id_for;
pub mod input;
pub mod input_trigger;
pub(crate) mod link_provider;
pub mod logging;
#[cfg(not(target_arch = "wasm32"))]
mod mcp_integrations;
pub mod memory_monitor;
pub mod mutable_tree;
pub mod navigation;
pub(crate) mod operation_matcher;
pub mod operations;
pub mod popup_menu;
pub mod preferences;
pub mod provider_cache;
pub mod reactive;
pub mod reactive_view;
pub mod reactive_view_model;
mod render_context;
pub mod render_interpreter;
pub mod rich_text_selection;
pub mod shadow_builders;
pub mod theme;
pub mod user_driver;
pub mod value_fns;
pub(crate) mod view_event_handler;
pub mod view_model;
pub mod widget_gallery;

// cdc module gutted — AppState, spawn_ui_listener, CdcState removed.
// Use reactive::ReactiveEngine instead.
pub use config::{HolonConfig, SessionConfig, UiConfig, WidgetState};
use holon_api::{EntityName, EntityUri};
pub use input::{InputAction, Key, WidgetInput};
#[cfg(not(target_arch = "wasm32"))]
pub use mcp_integrations::McpIntegrationRegistry;
pub use navigation::{
    CollectionNavigator, CursorHint, CursorPlacement, ListNavigator, NavDirection, NavTarget,
    TableNavigator, TreeNavigator,
};
pub use preferences::{PrefKey, PrefSection, PrefType, PreferenceDef};
pub use reactive::interpret_pure;
pub use reactive_view::{CollectionConfig, ReactiveView};
pub use reactive_view_model::{
    collection_variant_of, extract_item_template, variants_match, CollectionVariant, InterpretFn,
    ReactiveSlot, ReactiveViewModel,
};
pub use render_context::{AvailableSpace, LayoutHint, RenderContext};
pub use shadow_builders::DEFAULT_DRAWER_WIDTH;
pub use user_driver::{ReactiveEngineDriver, UserDriver};
pub use view_model::ViewModel;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use holon::api::BackendEngine;
use holon::sync::PublishErrorTracker;
pub use holon_api::UiInfo;

// Re-export types needed by consumers
pub use editor_controller::{EditorAction, EditorController, EditorKey};
pub use holon::api::backend_engine::QueryContext;
pub use holon::storage::turso::RowChangeStream;
pub use holon_api::{OperationDescriptor, ProviderAuthStatus, UiEvent, Value, WatcherCommand};
pub use operations::OperationIntent;
pub use reactive::LiveBlock;
pub use reactive::StubBuilderServices;

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
    #[cfg(not(target_arch = "wasm32"))]
    ready_signal: Option<holon_orgmode::di::FileWatcherReadySignal>,
    /// Extra services resolved from DI (for tests)
    extras: T,
    /// Keeps the background memory monitor alive (logs RSS every 30s)
    _memory_monitor: Option<memory_monitor::MemoryMonitorHandle>,
    /// Preference schema + theme registry, computed once at startup.
    preference_defs: Arc<Vec<preferences::PreferenceDef>>,
    theme_registry: Arc<theme::ThemeRegistry>,
    /// Unified config — runtime-mutable, persisted to holon.toml on changes.
    holon_config: Mutex<config::HolonConfig>,
    /// Config directory (where holon.toml lives).
    config_dir: PathBuf,
    /// Preference keys locked by CLI/env (read-only in UI).
    locked_keys: HashSet<preferences::PrefKey>,
}

impl FrontendSession<()> {
    /// Wrap an existing BackendEngine into a FrontendSession.
    ///
    /// Used by the PBT UI test: the PBT creates its own engine (via E2ESut),
    /// and this wraps it into a FrontendSession suitable for GLOBAL_SESSION
    /// so the Flutter app reuses the PBT's database.
    pub fn from_engine(engine: Arc<BackendEngine>) -> Self {
        let theme_registry = Arc::new(theme::ThemeRegistry::load(None));
        let preference_defs = Arc::new(preferences::define_preferences(&theme_registry));
        Self {
            engine,
            error_tracker: PublishErrorTracker::new(),
            #[cfg(not(target_arch = "wasm32"))]
            ready_signal: None,
            extras: (),
            _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
            preference_defs,
            theme_registry,
            holon_config: Mutex::new(config::HolonConfig::default()),
            config_dir: PathBuf::new(),
            locked_keys: HashSet::new(),
        }
    }
}

impl FrontendSession<()> {
    /// Create a new frontend session from a premortem-loaded `HolonConfig`.
    ///
    /// This is the preferred constructor. CLI frontends use `cli::build_session()`
    /// which calls this. Uses FluxDI to wire all services.
    pub async fn new_from_config(
        holon_config: config::HolonConfig,
        session_config: config::SessionConfig,
        config_dir: PathBuf,
        locked_keys: HashSet<preferences::PrefKey>,
    ) -> Result<Arc<Self>> {
        let (session, ()) = Self::new_from_config_with_di(
            holon_config,
            session_config,
            config_dir,
            locked_keys,
            |_| Ok(()),
            |_| (),
        )
        .await?;
        Ok(session)
    }

    /// Create a new frontend session with additional DI registrations.
    ///
    /// The `extra_setup` closure runs on the DI injector after `FrontendModule`
    /// is registered but before anything is resolved. Use it to register
    /// frontend-specific services (e.g. `set_render_interpreter`).
    ///
    /// The `extra_resolve` closure runs after session creation and can resolve
    /// additional services from the same DI container (e.g. `ReactiveEngine`).
    pub async fn new_from_config_with_di<F, G, T>(
        holon_config: config::HolonConfig,
        session_config: config::SessionConfig,
        config_dir: PathBuf,
        locked_keys: HashSet<preferences::PrefKey>,
        extra_setup: F,
        extra_resolve: G,
    ) -> Result<(Arc<Self>, T)>
    where
        F: FnOnce(&fluxdi::Injector) -> Result<()> + Send + 'static,
        G: FnOnce(&fluxdi::Injector) -> T + Send + 'static,
        T: Send + 'static,
    {
        use crate::frontend_module::FrontendInjectorExt;

        let db_path = holon_config.resolve_db_path(&config_dir);

        let (_engine, (session, extra)) = holon::di::create_backend_engine_with_extras(
            db_path,
            move |injector| {
                injector.add_frontend(holon_config, session_config, config_dir, locked_keys)?;
                extra_setup(injector)?;
                Ok(())
            },
            |injector| async move {
                let session = injector.resolve_async::<FrontendSession>().await;
                let extra = extra_resolve(&injector);
                (session, extra)
            },
        )
        .await?;

        Ok((session, extra))
    }
}

impl<T> FrontendSession<T> {
    /// Check if a preference is locked by CLI/env (read-only in UI).
    pub fn is_preference_locked(&self, key: &preferences::PrefKey) -> bool {
        self.locked_keys.contains(key)
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
        let (profile, _computed) = self.engine.profile_resolver().resolve_with_variants(row);
        Some(profile.as_ref().clone())
    }

    /// Read current UI config.
    pub fn ui_settings(&self) -> UiConfig {
        self.holon_config.lock().unwrap().ui.clone()
    }

    /// Mutate UI config and persist to disk.
    pub fn update_ui_settings(&self, f: impl FnOnce(&mut UiConfig)) {
        let mut guard = self.holon_config.lock().unwrap();
        f(&mut guard.ui);
        guard.save_runtime(&self.config_dir);
    }

    /// Look up widget state by block ID. Returns default (open=true) if not found.
    pub fn widget_state(&self, block_id: &str) -> WidgetState {
        self.holon_config
            .lock()
            .unwrap()
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
        let guard = self.holon_config.lock().unwrap();
        guard.get_preference(key).cloned().unwrap_or_else(|| {
            self.preference_defs
                .iter()
                .find(|d| d.key == *key)
                .map(|d| d.default.clone())
                .unwrap_or(toml::Value::String(String::new()))
        })
    }

    /// Set a preference value and persist to disk.
    pub fn set_preference(&self, key: &preferences::PrefKey, value: toml::Value) {
        let mut guard = self.holon_config.lock().unwrap();
        guard.set_preference(key, value);
        guard.save_runtime(&self.config_dir);
    }

    /// Generate the render data for the preferences UI.
    ///
    /// Returns a `RenderExpr` tree and data rows. Frontends interpret the
    /// expression with their existing `RenderInterpreter` / builder registry.
    pub fn preferences_render_data(
        &self,
    ) -> (
        holon_api::render_types::RenderExpr,
        Vec<Arc<HashMap<String, Value>>>,
    ) {
        let current = self.holon_config.lock().unwrap().preferences.clone();
        let expr = preferences::preferences_render_expr(&self.preference_defs);
        let rows =
            preferences::preferences_to_rows(&self.preference_defs, &current, &self.locked_keys);
        (expr, rows.into_iter().map(Arc::new).collect())
    }

    /// Generate the render data for the widget gallery.
    pub fn widget_gallery_render_data(
        &self,
    ) -> (
        holon_api::render_types::RenderExpr,
        Vec<Arc<HashMap<String, Value>>>,
    ) {
        (
            widget_gallery::widget_gallery_render_expr(),
            widget_gallery::widget_gallery_rows(),
        )
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

    /// Check if the file watcher has completed startup (success or failure).
    pub fn is_ready(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .ready_signal
            .as_ref()
            .map_or(true, |s| s.is_completed());
        #[cfg(target_arch = "wasm32")]
        true
    }

    // =========================================================================
    // Default Layout Seeding
    // =========================================================================

    fn default_doc_uri() -> holon_api::EntityUri {
        holon_api::EntityUri::no_parent()
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
    /// Seed a default layout from the bundled `index.org`.
    ///
    /// Available on native and wasm32-wasip1-threads (which has std::time and
    /// std::path). NOT available on wasm32-unknown-unknown (browser main thread)
    /// where the org parser's path/time dependencies are absent.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn seed_default_layout(engine: &BackendEngine) -> Result<()> {
        let db = engine.db_handle();

        // Seed fixed-ID document blocks from DEFAULT_ASSETS (idempotent via INSERT OR IGNORE).
        // Must run BEFORE the early return so it executes even when root layout exists.
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_millis() as i64;
            for asset in crate::DEFAULT_ASSETS {
                if let Some(doc_id) = asset.fixed_doc_id {
                    let title = asset
                        .filename
                        .strip_suffix(".org")
                        .unwrap_or(asset.filename);
                    db.execute(
                        &format!(
                            "INSERT OR IGNORE INTO block (id, parent_id, content, content_type, sort_key, properties, created_at, updated_at) \
                             VALUES ('{doc_id}', 'sentinel:no_parent', '{title}', 'text', 'A0', '{{}}', {now}, {now})"
                        ),
                        vec![],
                    )
                    .await?;
                    // Ensure the Page tag is present in the junction table
                    // (INSERT OR IGNORE skips the insert when the id collides,
                    // so any previously-seeded row would keep its prior tags).
                    db.execute(
                        &format!(
                            "INSERT OR IGNORE INTO block_tags (block_id, tag) VALUES ('{doc_id}', 'Page')"
                        ),
                        vec![],
                    )
                    .await?;
                }
            }
        }

        // Check if seed blocks already exist (idempotent: don't re-seed on restart).
        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID;
        let rows = db
            .query(
                &format!("SELECT id FROM block WHERE id = '{root_id}'"),
                HashMap::new(),
            )
            .await?;
        if !rows.is_empty() {
            // Seed blocks or real layout already exist — nothing to do.
            // If OrgSync later creates a real layout, the blocks with the same
            // IDs get upserted. Seed siblings under sentinel:no_parent remain
            // harmless orphans.
            return Ok(());
        }

        let content = include_str!("../../../assets/default/index.org");
        let path = Path::new("index.org");
        let root = Path::new("");
        let default_doc_uri = Self::default_doc_uri();
        let parse_result = holon_orgmode::parse_org_file(path, content, &default_doc_uri, root)?;

        // The parser generates doc URI from the file path (doc:index.org).
        // We need to rewrite top-level block parent_ids to our well-known doc URI.
        let file_doc_uri = parse_result.document.id.clone();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_millis() as i64;

        // Create the seeded page block (tags ⊇ ["Page"]). The first content
        // line is the title; we use `__default__` to match the existing fixture.
        db.execute(
            &format!(
                "INSERT OR IGNORE INTO block (id, parent_id, sort_key, content, properties, created_at, updated_at) \
                 VALUES ('{}', 'sentinel:no_parent', 'A0', '__default__', '{{}}', {}, {})",
                default_doc_uri, now, now
            ),
            vec![],
        )
        .await?;
        db.execute(
            &format!(
                "INSERT OR IGNORE INTO block_tags (block_id, tag) VALUES ('{}', 'Page')",
                default_doc_uri
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
            let mut block_tags: Vec<String> = Vec::new();

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
                } else if key == "tags" {
                    if let Value::Array(arr) = value {
                        for tag_val in arr {
                            if let Some(tag) = tag_val.as_string() {
                                block_tags.push(tag.to_string());
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

            for tag in &block_tags {
                let tag_sql = format!(
                    "INSERT OR IGNORE INTO block_tags (block_id, tag) VALUES ('{}', '{}')",
                    block.id.as_str().replace('\'', "''"),
                    tag.replace('\'', "''")
                );
                db.execute(&tag_sql, vec![]).await?;
            }
        }

        tracing::info!(
            "[FrontendSession] Seeded default layout ({} blocks)",
            parse_result.blocks.len()
        );
        Ok(())
    }

    // =========================================================================
    // Query Methods - These can only be called after initialization completes
    // =========================================================================

    /// Get the initial widget for the application root
    /// Watch a block's UI with automatic error recovery and structural hot-swap.
    ///
    /// Returns a long-lived stream of `UiEvent`s (Structure + Data) and a command
    /// channel for variant switching. Unlike `render_entity`, errors become
    /// `UiEvent::Structure` events with error WidgetSpecs — the stream stays open
    /// and recovers when the underlying block is fixed.
    pub async fn watch_ui(&self, block_id: &EntityUri) -> Result<holon_api::WatchHandle> {
        holon::api::watch_ui(Arc::clone(&self.engine), block_id.clone()).await
    }

    /// Execute a query and set up CDC streaming with enrichment.
    ///
    /// Returns an `EnrichedChangeStream` whose first batch contains the initial
    /// query results as `Change::Created` items, followed by CDC deltas.
    /// All rows are `EnrichedRow`: `properties` JSON is flattened to top-level
    /// keys and computed fields (from entity profile resolution) are injected.
    ///
    /// This is the canonical boundary where raw storage data enters the frontend.
    /// All reactive consumers (`ensure_query_watching`,
    /// `start_query`, frontend live_query builders) go through this method,
    /// ensuring uniform enrichment.
    pub async fn query_and_watch(
        &self,
        prql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<holon::api::ui_watcher::EnrichedChangeStream> {
        let raw = self.engine.query_and_watch(prql, params, context).await?;
        let resolver = self.engine.profile_resolver().clone();
        Ok(holon::api::ui_watcher::enrich_stream(raw, resolver))
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
        entity_name: &EntityName,
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
