//! @c4 component
//! @c4 layer Core
//! Pattern: MVVM ViewModel
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Frontend session abstraction and the MVVM **ViewModel** layer — owns the reactive `ReactiveViewModel` tree that the GPUI and TUI Views observe.
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

/// Re-exports for the cell primitive (defined in `holon-core`). Frontends
/// depend on `holon-frontend` already; this saves them adding a direct
/// `holon-core` dep just to import `Cell<String>` / `TextOp` / etc.
pub mod cell {
    pub use holon_core::cell::{
        compute_text_delta, Cell, CellBacking, CursorAnchor, CursorBias, DeltaOp,
        LwwTextCellBacking, TextCellBacking, TextDelta, TextOp,
    };
    pub use holon_core::cell_registry::{CellCache, EntityCellRegistry, EntityCellRegistryExt};
}

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
pub mod editor_view_model;
pub mod focus_path;
pub mod geometry;
pub mod render_services;
pub use geometry::{drawer_toggle_id_for, expand_toggle_id_for, vms_button_id_for};
pub mod editor_caret;
pub mod headless_editor_mirror;
pub mod input;
pub mod input_trigger;
pub(crate) mod link_provider;
pub mod logging;
pub mod memory_monitor;
pub mod mutable_tree;
pub mod navigation;
pub(crate) mod operation_matcher;
pub mod operations;
/// PBT SUT capability traits owned by the frontend (cap home-rule). Gated behind
/// the `pbt` feature so production builds never pull `holon-pbt-core`.
#[cfg(feature = "pbt")]
pub mod pbt_caps;
pub mod popup_menu;
pub mod preferences;
pub mod provider_cache;
pub mod reactive;
pub mod reactive_view;
pub mod reactive_view_model;
mod render_context;
pub mod render_interpreter;
pub mod rich_text_selection;
pub mod row_origin;
pub mod row_pipeline;
pub mod shadow_builders;
pub mod size_expectation;
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
pub use row_origin::{Occurrence, OccurrenceId, RowOrigin};
pub use shadow_builders::DEFAULT_DRAWER_WIDTH;
pub use user_driver::{ReactiveEngineDriver, UserDriver};
pub use view_model::ViewModel;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use holon_core::PublishErrorTracker;

pub use holon_api::UiInfo;

// Re-export types needed by consumers
pub use editor_view_model::{EditorAction, EditorKey, EditorViewModel};
pub use holon_api::QueryContext;
pub use holon_api::{OperationDescriptor, ProviderAuthStatus, UiEvent, Value, WatcherCommand};
pub use operations::OperationIntent;
pub use reactive::StubBuilderServices;
pub use reactive::{LiveBlock, WatchGuard};

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
    /// The query-execution capability (ADR 0004 Phase 9). `Some` only when the
    /// Turso query engine is wired (an upcast of its `BackendEngine`); `None` for
    /// a no-Turso (Loro-only) session, which renders from `block_query` instead.
    /// Consumers reach it through [`FrontendSession::query_engine`] and degrade
    /// visibly when it is absent rather than branching on the storage backend.
    query_engine: Option<Arc<dyn holon_api::QueryEngine>>,
    /// The block read seam (ADR 0004 Phase 9). Present in **both** wirings: the
    /// Turso session holds a `TursoBlockQuerySource` over its CDC mirrors, a
    /// no-Turso session a `LoroBlockQuerySource`. Consumers read blocks through
    /// this handle and never branch on which storage backend is wired.
    block_query: Arc<dyn holon_core::storage::BlockQuerySource>,
    /// The operation-execution capability (ADR 0004 Phase 9, Stage 4). Present in
    /// **both** wirings: the Turso session holds an upcast of its `BackendEngine`,
    /// a no-Turso session a `DispatchingOperationEngine` over Loro-native
    /// providers. `None` only when no operation capability is wired at all. The
    /// mutating paths route through here, never through `engine()`.
    operation_engine: Option<Arc<dyn holon_api::OperationEngine>>,
    /// The UI-render/watch capability (ADR 0004 Phase 9). Present in **both**
    /// wirings: a Turso session holds its `BackendEngine` (renders off CDC), a
    /// no-Turso session a `LoroUiWatcher` (renders from `block_query`). `watch_ui`
    /// dispatches through this capability with no backend branch.
    ui_watcher: Arc<dyn holon_api::UiWatcher>,
    /// The profile resolver — an `Arc<dyn ProfileResolving>`, present in **both**
    /// wirings (ADR 0004 Phase 9). Profile resolution is not a Turso-only
    /// capability: the Turso session populates this from `engine.profile_resolver()`,
    /// a no-Turso session from the bundled type registry. Consumers read profiles
    /// through this handle and never branch on which storage backend is wired.
    profiles: Arc<dyn holon_api::entity_profile::ProfileResolving>,
    error_tracker: PublishErrorTracker,
    ready_signal: Option<tokio::sync::watch::Receiver<Option<Result<(), String>>>>,
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

/// Everything a wiring crate (holon-app) supplies to construct a
/// [`FrontendSession`]: the five capabilities plus the session context.
/// Keeps the session's fields private while letting all backend assembly
/// live outside this crate (storage de-leak Stage 6).
pub struct SessionParts {
    pub query_engine: Option<Arc<dyn holon_api::QueryEngine>>,
    pub block_query: Arc<dyn holon_core::storage::BlockQuerySource>,
    pub operation_engine: Option<Arc<dyn holon_api::OperationEngine>>,
    pub ui_watcher: Arc<dyn holon_api::UiWatcher>,
    pub profiles: Arc<dyn holon_api::entity_profile::ProfileResolving>,
    pub error_tracker: PublishErrorTracker,
    pub ready_signal: Option<tokio::sync::watch::Receiver<Option<Result<(), String>>>>,
    pub preference_defs: Arc<Vec<preferences::PreferenceDef>>,
    pub theme_registry: Arc<theme::ThemeRegistry>,
    pub holon_config: config::HolonConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<preferences::PrefKey>,
}

impl SessionParts {
    /// Context defaults for a session constructed outside the config-driven
    /// DI path (e.g. the no-Turso wiring): freshly loaded theme + preference
    /// schema, default config, no ready signal.
    pub fn with_capabilities(
        query_engine: Option<Arc<dyn holon_api::QueryEngine>>,
        block_query: Arc<dyn holon_core::storage::BlockQuerySource>,
        operation_engine: Option<Arc<dyn holon_api::OperationEngine>>,
        ui_watcher: Arc<dyn holon_api::UiWatcher>,
        profiles: Arc<dyn holon_api::entity_profile::ProfileResolving>,
    ) -> Self {
        let theme_registry = Arc::new(theme::ThemeRegistry::load(None));
        let preference_defs = Arc::new(preferences::define_preferences(&theme_registry));
        Self {
            query_engine,
            block_query,
            operation_engine,
            ui_watcher,
            profiles,
            error_tracker: PublishErrorTracker::new(),
            ready_signal: None,
            preference_defs,
            theme_registry,
            holon_config: config::HolonConfig::default(),
            config_dir: PathBuf::new(),
            locked_keys: HashSet::new(),
        }
    }
}

impl FrontendSession<()> {
    /// Construct a session from pre-assembled capabilities. The only public
    /// constructor — all backend assembly (Turso DI, no-Turso Loro stack)
    /// lives in the wiring crate and funnels through here.
    pub fn from_parts(parts: SessionParts) -> Self {
        Self {
            query_engine: parts.query_engine,
            block_query: parts.block_query,
            operation_engine: parts.operation_engine,
            ui_watcher: parts.ui_watcher,
            profiles: parts.profiles,
            error_tracker: parts.error_tracker,
            ready_signal: parts.ready_signal,
            extras: (),
            _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
            preference_defs: parts.preference_defs,
            theme_registry: parts.theme_registry,
            holon_config: Mutex::new(parts.holon_config),
            config_dir: parts.config_dir,
            locked_keys: parts.locked_keys,
        }
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

    /// The block read seam, present in **both** wirings (ADR 0004 Phase 9).
    /// Consumers capture a [`BlockSnapshot`](holon_core::storage::BlockSnapshot)
    /// via `snapshot()` and never branch on the storage backend.
    pub fn block_query(&self) -> &Arc<dyn holon_core::storage::BlockQuerySource> {
        &self.block_query
    }

    /// The profile resolver, available in **both** wirings (ADR 0004 Phase 9).
    ///
    /// Profile resolution is not a Turso-only capability: a no-Turso session
    /// builds this from the bundled type registry, and the Turso session reuses
    /// `engine().profile_resolver()`. The render path reads profiles through here
    /// so it never panics for lack of an engine.
    pub fn profiles(&self) -> &Arc<dyn holon_api::entity_profile::ProfileResolving> {
        &self.profiles
    }

    /// The query-execution capability (ADR 0004 — "Turso is one of four").
    ///
    /// `Some` only when the Turso query engine is wired; `None` for a no-Turso
    /// (Loro-only) session. The frontend's query path depends on this capability
    /// rather than the concrete `BackendEngine`, and degrades visibly (query
    /// blocks show their `source`) when it is absent — never panicking.
    pub fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        self.query_engine.clone()
    }

    /// The operation-execution capability (ADR 0004 — "Turso is one of four").
    ///
    /// Covers dispatching operations, operation discovery, and undo/redo. Present
    /// in both wirings — the Turso session upcasts its `BackendEngine`, a no-Turso
    /// session holds a `DispatchingOperationEngine` over Loro-native providers
    /// (Stage 4). `None` only when no operation capability is wired. Callers route
    /// through this rather than `engine()`, surfacing absence as a typed error.
    pub fn operation_engine(&self) -> Option<Arc<dyn holon_api::OperationEngine>> {
        self.operation_engine.clone()
    }

    /// The operation engine, or a typed error when none is wired. Used by the
    /// mutating operation paths (dispatch, undo, redo) that must fail loud rather
    /// than silently no-op when the capability is absent.
    fn require_operation_engine(&self) -> Result<Arc<dyn holon_api::OperationEngine>> {
        self.operation_engine().ok_or_else(|| {
            anyhow::anyhow!(
                "this operation requires an operation engine, which is not wired in this (no-Turso) session"
            )
        })
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
    ) -> Option<holon_api::RenderProfile> {
        let (profile, _computed) = self.profiles().resolve_with_variants(row);
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
        self.widget_state_explicit(block_id).unwrap_or_default()
    }

    /// Look up the *explicitly stored* widget state, or `None` when the user has
    /// never toggled this widget. Callers that need a mode-aware default (e.g.
    /// overlay drawers, which must start closed) use this instead of
    /// [`Self::widget_state`], whose `unwrap_or_default` collapses "never set"
    /// into `open = true`.
    pub fn widget_state_explicit(&self, block_id: &str) -> Option<WidgetState> {
        self.holon_config
            .lock()
            .unwrap()
            .ui
            .widgets
            .get(block_id)
            .cloned()
    }

    /// Effective open state for a drawer in the given mode. Mirrors
    /// [`crate::reactive::BuilderServices::drawer_open`] for callers that hold a
    /// `FrontendSession` directly (e.g. the GPUI toolbar toggles): explicit user
    /// state wins; otherwise `Overlay` drawers default closed, `Shrink` open.
    pub fn drawer_open(&self, block_id: &str, mode: crate::view_model::DrawerMode) -> bool {
        self.widget_state_explicit(block_id)
            .map(|s| s.open)
            .unwrap_or_else(|| mode.default_open())
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
        self.ready_signal
            .as_ref()
            .map_or(true, |rx| rx.borrow().is_some())
    }

    // =========================================================================
    // Default Layout Seeding
    // =========================================================================

    pub fn default_doc_uri() -> holon_api::EntityUri {
        // A real block id, NOT the `sentinel:no_parent` marker. Using the
        // sentinel here made the `__default__` doc a self-referential block
        // (`id == parent == sentinel:no_parent`) that could never be a Loro
        // node — it mangled to `block:no_parent` on any Loro round-trip and
        // tripped `prepare_delete`'s cascade. The `__default__` page is hidden
        // from the Pages sidebar by an explicit `b.id != 'block:__default__'`
        // filter in `index.org`.
        holon_api::default_doc_block_uri()
    }

    /// Seed a default layout into the database if no real layout exists.
    ///
    /// On fresh installations (no org directory), the app needs a root layout
    /// to render the 3-column UI. This parses the bundled `index.org` and
    /// creates blocks under the well-known `block:__default__` page.
    ///
    /// When a real `index.org` is later synced, the next startup detects the
    /// real layout and cleans up the seeded blocks.
    ///
    /// Uses raw SQL via db_handle because OperationProviders may not be
    /// registered (e.g. TUI without orgmode). This is a bootstrap operation
    /// that doesn't need events, undo, or observers.
    /// Build the default-layout seed blocks — the fixed-document-page shells
    /// (e.g. `block:journals`) and the `__default__` page. Always returns
    /// these. Callers that want the full `index.org` layout must append the
    /// parsed org blocks separately (see `holon-app/src/seed.rs`).
    pub fn build_default_layout_blocks(fresh: bool) -> Result<Vec<holon_api::block::Block>> {
        use holon_api::block::Block;

        let default_doc_uri = Self::default_doc_uri();
        let mut entries: Vec<Block> = Vec::new();

        // Fixed-id document pages (e.g. block:journals). Always built so a
        // missing page shell is repaired; persisting is idempotent.
        for asset in crate::DEFAULT_ASSETS {
            if let Some(doc_id) = asset.fixed_doc_id {
                let title = asset
                    .filename
                    .strip_suffix(".org")
                    .unwrap_or(asset.filename);
                let mut page = Block::new_text(
                    // ALLOW(entity_uri_from_raw): static asset literal asset.fixed_doc_id
                    EntityUri::from_raw(doc_id),
                    EntityUri::no_parent(),
                    title.to_string(),
                );
                page.set_page(true);
                entries.push(page);
            }
        }

        if fresh {
            // The `__default__` page that owns the 3-column layout.
            let mut def_page = Block::new_text(
                default_doc_uri.clone(),
                EntityUri::no_parent(),
                "__default__".to_string(),
            );
            def_page.set_page(true);
            entries.push(def_page);
        }

        Ok(entries)
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
        // Dispatch through the `UiWatcher` capability — `BackendEngine` (CDC) for
        // Turso, `LoroUiWatcher` (block_query snapshot) for no-Turso. No branch.
        self.ui_watcher.clone().watch_ui(block_id.clone()).await
    }

    /// Compile a query (PRQL/GQL/SQL) and set up CDC streaming with enrichment.
    ///
    /// Returns an `EnrichedChangeStream` whose first batch contains the initial
    /// query results as `Change::Created` items, followed by CDC deltas.
    /// All rows are `EnrichedRow`: `properties` JSON is flattened to top-level
    /// keys and computed fields (from entity profile resolution) are injected.
    ///
    /// SQL compilation and enrichment both happen behind the `QueryEngine`
    /// capability — this layer never sees SQL strings or the raw Turso stream.
    /// All reactive consumers (`ensure_query_watching`, frontend live_query
    /// builders) go through this method, ensuring uniform enrichment.
    pub async fn watch_query(
        &self,
        query: &str,
        language: holon_api::QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        self.query_engine()
            .ok_or_else(|| anyhow::anyhow!("watch_query requires the Turso query engine, which is not wired in this (no-Turso) session"))?
            .watch_query(query, language, params, context)
            .await
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
        self.require_operation_engine()?
            .execute_operation(
                entity_name,
                op_name,
                params.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            )
            .await
    }

    /// Get available operations for an entity
    ///
    /// Returns a list of operation descriptors available for the given entity_name.
    /// Use "*" as entity_name to get wildcard operations. Empty when no operation
    /// engine is wired (a no-Turso session has no operations to offer yet).
    pub async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        match self.operation_engine() {
            Some(ops) => ops.available_operations(entity_name).await,
            None => Vec::new(),
        }
    }

    /// Check if an operation is available for an entity
    pub async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        match self.operation_engine() {
            Some(ops) => ops.has_operation(entity_name, op_name).await,
            None => false,
        }
    }

    /// Undo the last operation
    ///
    /// Returns true if an operation was undone, false if the undo stack is empty.
    pub async fn undo(&self) -> Result<bool> {
        self.require_operation_engine()?.undo().await
    }

    /// Redo the last undone operation
    ///
    /// Returns true if an operation was redone, false if the redo stack is empty.
    pub async fn redo(&self) -> Result<bool> {
        self.require_operation_engine()?.redo().await
    }

    /// Check if undo is available
    pub async fn can_undo(&self) -> bool {
        match self.operation_engine() {
            Some(ops) => ops.can_undo().await,
            None => false,
        }
    }

    /// Check if redo is available
    pub async fn can_redo(&self) -> bool {
        match self.operation_engine() {
            Some(ops) => ops.can_redo().await,
            None => false,
        }
    }

    /// Look up a block's path from the blocks_with_paths materialized view
    ///
    /// Returns the hierarchical path for a block (e.g., "/parent/block_id").
    /// This path is used for descendants queries via path prefix matching.
    pub async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        self.query_engine()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "lookup_block_path requires the Turso query engine, which is not wired in this (no-Turso) session"
                )
            })?
            .lookup_block_path(block_id)
            .await
    }
}
