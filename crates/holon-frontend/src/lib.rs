//! @c4 component
//! @c4 layer Core
//! Pattern: MVVM ViewModel
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Frontend session abstraction and the MVVM **ViewModel** layer — owns the
//! reactive `ReactiveViewModel` tree that the GPUI and TUI Views observe.
//!
//! Uses `premortem` for layered config (Defaults → TOML → CLI/env) and `clap`
//! for CLI parsing. Configuration is defined once in [`config::HolonConfig`]
//! and automatically gets CLI + env var + TOML file support.
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

pub mod advice_weaver;
pub mod cdc;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod collection_layout;
pub mod lane_filtered_provider;

/// Re-exports for the cell primitive (defined in `holon-core`). Frontends
/// depend on `holon-frontend` already; this saves them adding a direct
/// `holon-core` dep just to import `Cell<String>` / `TextOp` / etc.
pub mod cell {
    pub use holon_core::cell::Cell;
    pub use holon_core::cell::CellBacking;
    pub use holon_core::cell::CursorAnchor;
    pub use holon_core::cell::CursorBias;
    pub use holon_core::cell::DeltaOp;
    pub use holon_core::cell::LwwTextCellBacking;
    pub use holon_core::cell::TextCellBacking;
    pub use holon_core::cell::TextDelta;
    pub use holon_core::cell::TextOp;
    pub use holon_core::cell::compute_text_delta;
    pub use holon_core::cell_registry::CellCache;
    pub use holon_core::cell_registry::EntityCellRegistry;
    pub use holon_core::cell_registry::EntityCellRegistryExt;
}

/// A default org file bundled with the app, seeded to disk on first launch
/// (empty vault). The journals page is NOT seeded this way — it is built
/// programmatically as blocks (see [`journals_page_blocks`]) so its
/// query/render/ auto-create machinery lives directly under the fixed
/// `block:journals` shell with no separate org document (which would mint a
/// duplicate "Journals" page).
pub struct DefaultAsset {
    pub filename: &'static str,
    pub content: &'static str,
    /// Fixed document block ID. Enables org content to reference its own
    /// document (e.g., `parent_id == 'block:journals'`). `None` means
    /// random UUID.
    pub fixed_doc_id: Option<&'static str>,
}

/// Org files seeded to disk when the vault has no `.org` files. Empty: the sole
/// former entry (`Journals.org`) is now seeded programmatically via
/// [`journals_page_blocks`] to avoid the duplicate-page defect (a disk
/// `Journals.org` with no `#+ID:` parsed to a second `file:` page carrying the
/// machinery, while `build_default_layout_blocks` minted a bare
/// `block:journals` shell — two "Journals" Pages). Retained as a seam for
/// future disk-seeded assets.
pub const DEFAULT_ASSETS: &[DefaultAsset] = &[];

/// Deterministic block ids for the journals page. Seeded on every boot so
/// re-seeding is a no-op (never a duplicate).
pub const JOURNALS_PAGE_ID: &str = "block:journals";
pub const JOURNALS_SRC_ID: &str = "block:journals::src::0";
pub const JOURNALS_RENDER_ID: &str = "block:journals::render::0";
pub const JOURNALS_AUTO_CREATE_ID: &str = "block:journals::auto-create";
/// The single-block `holon_rule` (ADR 0024 §7.2) that auto-creates today's
/// journal. Named `::action::0` to match the ratified
/// `assets/default/Journals.org` asset (and so its `RuleId` — the block id — is
/// stable across the disk/programmatic seed paths).
pub const JOURNALS_ACTION_ID: &str = "block:journals::action::0";

/// The `block:journals` page, as programmatic blocks (no org document). The
/// shell page owns, directly, its display query (`::src::0`, holon_prql listing
/// the journal day-entries) and render (`::render::0`). All ids are
/// deterministic, so seeding this on every boot is idempotent: create-if-absent
/// never duplicates and never clobbers user edits.
///
/// The auto-create RULE (trigger + action) is intentionally NOT here — see
/// [`journals_auto_create_blocks`] for why it cannot yet be co-located on the
/// journals landing page.
pub fn journals_page_blocks() -> Vec<holon_api::block::Block> {
    use holon_api::block::Block;

    let uri = |raw: &str| EntityUri::parse(raw).expect("static journals block id");
    let journals = uri(JOURNALS_PAGE_ID);

    let mut page = Block::new_text(journals.clone(), EntityUri::no_parent(), "Journals");
    page.set_page(true);

    // holon_sql (not prql): the recursive descendant expansion and the
    // `expand_default` marker are simplest expressed in SQL. Only `Page`-tagged
    // day-entries (via the block_tags junction), newest-first. `1 AS
    // expand_default` tags every feed row so `render_entity` routes it to the
    // `embedded_page_expanded` profile variant (default-expanded), not the
    // collapsed `embedded_page`.
    let src = Block::new_source(
        uri(JOURNALS_SRC_ID),
        journals.clone(),
        "holon_sql",
        concat!(
            "SELECT b.*, 1 AS expand_default FROM block b ",
            "JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' ",
            "WHERE b.parent_id = 'block:journals' ORDER BY b.content DESC",
        ),
    );

    // LogSeq-style feed: each day-entry rendered as a default-EXPANDED embedded
    // page (via the `embedded_page_expanded` variant, keyed on `expand_default`),
    // separated by a `divider()`. `render_entity()` per row keeps the embedded
    // page-boundary + lazy-descendant semantics (children load on materialise).
    let render = Block::new_source(
        uri(JOURNALS_RENDER_ID),
        journals,
        "render",
        r#"list(#{sortkey: "-content", item_template: column(render_entity(), divider())})"#,
    );

    vec![page, src, render]
}

/// The journal auto-create RULE: a "Journal Auto-Create" heading owning a
/// SINGLE `holon_rule` YAML block (ADR 0024 §7.2 ratified form — guard + emit
/// in one block, matching `assets/default/Journals.org`). The
/// `holon_rule_watcher` discovers it (the legacy `holon_sql`-trigger +
/// `block.create`-action pairing is gone — no sibling source, so the old
/// `action_watcher` leaves it alone), reads the `clock` day for its `{today}`
/// binding, evaluates the `when:` inhibitor, and emits today's journal under
/// the journals page via a WP2 deterministic id.
///
/// Seeded on every boot by [`FrontendSession::build_default_layout_blocks`]
/// (dogfood #4 fix, 2026-07-12): the clock-day trigger fires the action so real
/// vaults get today's journal. The prior render-panic blocker is resolved — the
/// trigger/action are `is_program` blocks (fork-A: `rule_sibling` profile
/// exclusion + `RowIdentity`-keyed reactive rows), so they are never
/// display-evaluated as a collection and the id-less trigger row no longer
/// panics the render worker. The end-to-end firing (clock scheduler seeds the
/// `clock` day row → trigger matview → action watcher → deterministic-id
/// `block.create`) is pinned by the directed capstone
/// `advance_day_fires_one_journal_per_distinct_day_idempotently` and, in the
/// composed keystone, by the fixed-clock boot-journal model in `wide_e2e`.
pub fn journals_auto_create_blocks() -> Vec<holon_api::block::Block> {
    use holon_api::block::Block;

    let uri = |raw: &str| EntityUri::parse(raw).expect("static journals block id");
    let journals = uri(JOURNALS_PAGE_ID);
    let auto_create = uri(JOURNALS_AUTO_CREATE_ID);

    let auto = Block::new_text(auto_create.clone(), journals, "Journal Auto-Create");

    // Single-block holon_rule (sugar form): `when:` = clock-day inhibitor arc
    // (`not block_exists("Journals/{today}")`), `emit:` = ratcheted create of the
    // `{today}` block under the journals page. Byte-identical to the ratified
    // `assets/default/Journals.org` rule so the disk and programmatic seeds agree.
    let rule = Block::new_source(
        uri(JOURNALS_ACTION_ID),
        auto_create,
        "holon_rule",
        // No trailing newline: the block store normalizes it away (like every
        // other seed block), so the reference — which models this exact content —
        // must match `block_raw`/sql without one.
        "name: daily_journal\nwhen: 'not block_exists(\"Journals/{today}\")'\nemit:\n  place: \
         journals\n  name: \"{today}\"",
    );

    vec![auto, rule]
}
pub mod command_provider;
pub mod config;
pub mod editor_view_model;
pub mod focus_path;
pub mod geometry;
pub mod render_services;
pub use geometry::drawer_toggle_id_for;
pub use geometry::expand_toggle_id_for;
pub use geometry::vms_button_id_for;
pub mod editor_caret;
pub mod headless_editor_mirror;
pub mod input;
pub mod input_trigger;
pub(crate) mod link_provider;
pub mod link_segments;
pub mod logging;
pub mod memory_monitor;
pub mod mutable_tree;
pub mod navigation;
pub(crate) mod operation_matcher;
pub mod operations;
/// PBT SUT capability traits owned by the frontend (cap home-rule). Gated
/// behind the `pbt` feature so production builds never pull `holon-pbt-core`.
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
pub mod template_placement;
pub mod theme;
pub mod tour;
pub mod user_driver;
pub mod value_fns;
pub(crate) mod view_event_handler;
pub mod view_model;
pub mod widget_gallery;

// cdc module gutted — AppState, spawn_ui_listener, CdcState removed.
// Use reactive::ReactiveEngine instead.
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
pub use config::HolonConfig;
pub use config::SessionConfig;
pub use config::UiConfig;
pub use config::WidgetState;
// Re-export types needed by consumers
pub use editor_view_model::{EditorAction, EditorKey, EditorViewModel};
use holon_api::EntityName;
use holon_api::EntityUri;
pub use holon_api::OperationDescriptor;
pub use holon_api::ProviderAuthStatus;
pub use holon_api::QueryContext;
pub use holon_api::UiEvent;
pub use holon_api::UiInfo;
pub use holon_api::Value;
pub use holon_api::WatcherCommand;
use holon_core::PublishErrorTracker;
pub use input::InputAction;
pub use input::Key;
pub use input::WidgetInput;
pub use navigation::CollectionNavigator;
pub use navigation::CursorHint;
pub use navigation::CursorPlacement;
pub use navigation::ListNavigator;
pub use navigation::NavDirection;
pub use navigation::NavTarget;
pub use navigation::TableNavigator;
pub use navigation::TreeNavigator;
pub use operations::OperationIntent;
pub use preferences::PrefKey;
pub use preferences::PrefSection;
pub use preferences::PrefType;
pub use preferences::PreferenceDef;
pub use reactive::LiveBlock;
pub use reactive::StubBuilderServices;
pub use reactive::WatchGuard;
pub use reactive::interpret_pure;
pub use reactive_view::CollectionConfig;
pub use reactive_view::ReactiveView;
pub use reactive_view_model::CollectionVariant;
pub use reactive_view_model::InterpretFn;
pub use reactive_view_model::ReactiveSlot;
pub use reactive_view_model::ReactiveViewModel;
pub use reactive_view_model::collection_variant_of;
pub use reactive_view_model::extract_item_template;
pub use reactive_view_model::variants_match;
pub use render_context::AvailableSpace;
pub use render_context::LayoutHint;
pub use render_context::RenderContext;
pub use row_origin::Occurrence;
pub use row_origin::OccurrenceId;
pub use row_origin::RowOrigin;
pub use shadow_builders::DEFAULT_DRAWER_WIDTH;
pub use user_driver::ReactiveEngineDriver;
pub use user_driver::UserDriver;
pub use view_model::ViewModel;

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
    /// Turso query engine is wired (an upcast of its `BackendEngine`); `None`
    /// for a no-Turso (Loro-only) session, which renders from `block_query`
    /// instead. Consumers reach it through
    /// [`FrontendSession::query_engine`] and degrade visibly when it is
    /// absent rather than branching on the storage backend.
    query_engine: Option<Arc<dyn holon_api::QueryEngine>>,
    /// The block read seam (ADR 0004 Phase 9). Present in **both** wirings: the
    /// Turso session holds a `TursoBlockQuerySource` over its CDC mirrors, a
    /// no-Turso session a `LoroBlockQuerySource`. Consumers read blocks through
    /// this handle and never branch on which storage backend is wired.
    block_query: Arc<dyn holon_core::storage::BlockQuerySource>,
    /// The operation-execution capability (ADR 0004 Phase 9, Stage 4). Present
    /// in **both** wirings: the Turso session holds an upcast of its
    /// `BackendEngine`, a no-Turso session a `DispatchingOperationEngine`
    /// over Loro-native providers. `None` only when no operation capability
    /// is wired at all. The mutating paths route through here, never
    /// through `engine()`.
    operation_engine: Option<Arc<dyn holon_api::OperationEngine>>,
    /// The UI-render/watch capability (ADR 0004 Phase 9). Present in **both**
    /// wirings: a Turso session holds its `BackendEngine` (renders off CDC), a
    /// no-Turso session a `LoroUiWatcher` (renders from `block_query`).
    /// `watch_ui` dispatches through this capability with no backend
    /// branch.
    ui_watcher: Arc<dyn holon_api::UiWatcher>,
    /// The profile resolver — an `Arc<dyn ProfileResolving>`, present in
    /// **both** wirings (ADR 0004 Phase 9). Profile resolution is not a
    /// Turso-only capability: the Turso session populates this from
    /// `engine.profile_resolver()`, a no-Turso session from the bundled
    /// type registry. Consumers read profiles through this handle and never
    /// branch on which storage backend is wired.
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
    /// Consumers capture a
    /// [`BlockSnapshot`](holon_core::storage::BlockSnapshot)
    /// via `snapshot()` and never branch on the storage backend.
    pub fn block_query(&self) -> &Arc<dyn holon_core::storage::BlockQuerySource> {
        &self.block_query
    }

    /// The profile resolver, available in **both** wirings (ADR 0004 Phase 9).
    ///
    /// Profile resolution is not a Turso-only capability: a no-Turso session
    /// builds this from the bundled type registry, and the Turso session reuses
    /// `engine().profile_resolver()`. The render path reads profiles through
    /// here so it never panics for lack of an engine.
    pub fn profiles(&self) -> &Arc<dyn holon_api::entity_profile::ProfileResolving> {
        &self.profiles
    }

    /// The query-execution capability (ADR 0004 — "Turso is one of four").
    ///
    /// `Some` only when the Turso query engine is wired; `None` for a no-Turso
    /// (Loro-only) session. The frontend's query path depends on this
    /// capability rather than the concrete `BackendEngine`, and degrades
    /// visibly (query blocks show their `source`) when it is absent — never
    /// panicking.
    pub fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        self.query_engine.clone()
    }

    /// The operation-execution capability (ADR 0004 — "Turso is one of four").
    ///
    /// Covers dispatching operations, operation discovery, and undo/redo.
    /// Present in both wirings — the Turso session upcasts its
    /// `BackendEngine`, a no-Turso session holds a
    /// `DispatchingOperationEngine` over Loro-native providers (Stage 4).
    /// `None` only when no operation capability is wired. Callers route
    /// through this rather than `engine()`, surfacing absence as a typed error.
    pub fn operation_engine(&self) -> Option<Arc<dyn holon_api::OperationEngine>> {
        self.operation_engine.clone()
    }

    /// The operation engine, or a typed error when none is wired. Used by the
    /// mutating operation paths (dispatch, undo, redo) that must fail loud
    /// rather than silently no-op when the capability is absent.
    fn require_operation_engine(&self) -> Result<Arc<dyn holon_api::OperationEngine>> {
        self.operation_engine().ok_or_else(|| {
            anyhow::anyhow!(
                "this operation requires an operation engine, which is not wired in this \
                 (no-Turso) session"
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

    /// The config directory (where `holon.toml` and other app-local state live).
    pub fn config_dir(&self) -> &std::path::Path {
        &self.config_dir
    }

    /// Mutate UI config and persist to disk.
    pub fn update_ui_settings(&self, f: impl FnOnce(&mut UiConfig)) {
        let mut guard = self.holon_config.lock().unwrap();
        f(&mut guard.ui);
        guard.save_runtime(&self.config_dir);
    }

    /// Look up widget state by block ID. Returns default (open=true) if not
    /// found.
    pub fn widget_state(&self, block_id: &str) -> WidgetState {
        self.widget_state_explicit(block_id).unwrap_or_default()
    }

    /// Look up the *explicitly stored* widget state, or `None` when the user
    /// has never toggled this widget. Callers that need a mode-aware
    /// default (e.g. overlay drawers, which must start closed) use this
    /// instead of [`Self::widget_state`], whose `unwrap_or_default`
    /// collapses "never set" into `open = true`.
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
    /// [`crate::reactive::BuilderServices::drawer_open`] for callers that hold
    /// a `FrontendSession` directly (e.g. the GPUI toolbar toggles):
    /// explicit user state wins; otherwise `Overlay` drawers default
    /// closed, `Shrink` open.
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

    /// Read a preference value. Returns the stored value or the definition's
    /// default.
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

        // The `block:journals` page and its machinery, seeded programmatically
        // (no org document). Always built so both an empty AND an already-seeded
        // vault get the journal auto-create infrastructure; persisting is
        // idempotent (deterministic ids), so a re-seed never duplicates the page
        // and never clobbers user-created journal entries.
        entries.extend(crate::journals_page_blocks());
        // The auto-create RULE (trigger + action). Seeded on every boot so the
        // clock-day trigger fires `block.create` and the vault gets today's
        // journal (dogfood #4: real vaults never got one). The trigger/action are
        // `is_program` blocks — profile routing keeps them out of display-query
        // evaluation, so they never render as a collection (fork-A safeguards).
        entries.extend(crate::journals_auto_create_blocks());

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
    /// Watch a block's UI with automatic error recovery and structural
    /// hot-swap.
    ///
    /// Returns a long-lived stream of `UiEvent`s (Structure + Data) and a
    /// command channel for variant switching. Unlike `render_entity`,
    /// errors become `UiEvent::Structure` events with error WidgetSpecs —
    /// the stream stays open and recovers when the underlying block is
    /// fixed.
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
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "watch_query requires the Turso query engine, which is not wired in this \
                     (no-Turso) session"
                )
            })?
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
        // A `FrontendSession` is, by construction, a user session: every op it
        // dispatches is a direct user gesture. System-authored ops (rule
        // firings, CRDT sync, org ingest) never route through here — they call
        // the engine / providers directly with their own `OpOrigin`.
        self.require_operation_engine()?
            .execute_operation(
                entity_name,
                op_name,
                params.into_iter().map(|(k, v)| (k.into(), v)).collect(),
                holon_api::OpOrigin::User,
            )
            .await
    }

    /// Get available operations for an entity
    ///
    /// Returns a list of operation descriptors available for the given
    /// entity_name. Use "*" as entity_name to get wildcard operations.
    /// Empty when no operation engine is wired (a no-Turso session has no
    /// operations to offer yet).
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

    /// Undo the last operation. See [`holon_api::UndoOutcome`] — a stale entry
    /// is dropped with `StaleDropped` (surfaceable) rather than silently
    /// skipped.
    pub async fn undo(&self) -> Result<holon_api::UndoOutcome> {
        self.require_operation_engine()?.undo().await
    }

    /// Redo the last undone operation. See [`holon_api::UndoOutcome`].
    pub async fn redo(&self) -> Result<holon_api::UndoOutcome> {
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
                    "lookup_block_path requires the Turso query engine, which is not wired in \
                     this (no-Turso) session"
                )
            })?
            .lookup_block_path(block_id)
            .await
    }
}

#[cfg(test)]
mod journals_seed_tests {
    use holon_api::ContentType;
    use holon_api::SourceLanguage;
    use holon_api::block::Block;

    use super::*;

    fn find<'a>(blocks: &'a [Block], id: &str) -> &'a Block {
        blocks
            .iter()
            .find(|b| b.id.as_str() == id)
            .unwrap_or_else(|| panic!("block {id} missing from {:?}", ids(blocks)))
    }

    fn ids(blocks: &[Block]) -> Vec<String> {
        blocks.iter().map(|b| b.id.as_str().to_string()).collect()
    }

    #[test]
    fn journals_page_blocks_shell_owns_query_and_render() {
        let blocks = journals_page_blocks();

        // The page shell + its display query/render, owned directly by the shell
        // (no separate org document) — the fix for the duplicate-page defect.
        let page = find(&blocks, JOURNALS_PAGE_ID);
        assert!(page.is_page(), "block:journals is the Page shell");
        assert_eq!(page.parent_id, EntityUri::no_parent());

        // Display query + render are DIRECT children of the shell so the render
        // system resolves them for the journals page.
        for id in [JOURNALS_SRC_ID, JOURNALS_RENDER_ID] {
            assert_eq!(
                find(&blocks, id).parent_id.as_str(),
                JOURNALS_PAGE_ID,
                "{id} is a direct child of block:journals"
            );
        }
        assert_eq!(
            find(&blocks, JOURNALS_SRC_ID).source_language,
            Some(SourceLanguage::Query(holon_api::QueryLanguage::HolonSql))
        );
        assert!(matches!(
            find(&blocks, JOURNALS_RENDER_ID).source_language,
            Some(SourceLanguage::Render)
        ));

        // `journals_page_blocks` is the page-display spec only: the auto-create
        // rule is a separate spec (`journals_auto_create_blocks`), both seeded by
        // `build_default_layout_blocks`. The page-display spec carries no rule.
        assert!(
            !blocks.iter().any(|b| b.id.as_str() == JOURNALS_ACTION_ID),
            "the page-display spec carries no rule (it lives in the rule spec)"
        );
    }

    #[test]
    fn journals_auto_create_is_a_single_block_holon_rule() {
        let blocks = journals_auto_create_blocks();
        // A "Journal Auto-Create" heading hosting ONE self-contained holon_rule
        // block (ADR 0024 §7.2) — no legacy holon_sql trigger sibling.
        let auto = find(&blocks, JOURNALS_AUTO_CREATE_ID);
        assert_eq!(auto.parent_id.as_str(), JOURNALS_PAGE_ID);
        // Exactly two blocks: the heading + the rule. No holon_sql trigger sibling.
        assert_eq!(
            blocks.len(),
            2,
            "single-block rule: heading + one holon_rule"
        );
        assert!(
            blocks
                .iter()
                .filter(|b| b.content_type == ContentType::Source)
                .all(|b| b.source_language == Some(SourceLanguage::HolonRule)),
            "the only source block is the holon_rule (no holon_sql trigger)"
        );
        let rule = find(&blocks, JOURNALS_ACTION_ID);
        assert_eq!(rule.parent_id.as_str(), JOURNALS_AUTO_CREATE_ID);
        assert!(matches!(
            rule.source_language,
            Some(SourceLanguage::HolonRule)
        ));
        // The ratified sugar form: `when:` guard + `emit:` place/name.
        assert!(rule.content.contains("when:") && rule.content.contains("emit:"));
        assert!(rule.content.contains("place: journals"));
    }

    #[test]
    fn journals_page_blocks_are_deterministic_and_idempotent() {
        // Same ids on every call — so seeding on every boot upserts (never
        // duplicates) the journal infrastructure.
        assert_eq!(ids(&journals_page_blocks()), ids(&journals_page_blocks()));
    }

    #[test]
    fn default_layout_includes_journal_machinery_on_every_boot() {
        // Defect 2 (partial): a non-empty (already-seeded / org) vault boots with
        // fresh=false but MUST still get the journals page + its display query, so
        // no vault is left without journal infrastructure. Both the fresh and the
        // non-fresh layout carry the page.
        for fresh in [true, false] {
            let entries =
                FrontendSession::<()>::build_default_layout_blocks(fresh).expect("build layout");
            let entry_ids: std::collections::HashSet<String> =
                entries.iter().map(|b| b.id.as_str().to_string()).collect();
            for id in [
                JOURNALS_PAGE_ID,
                JOURNALS_SRC_ID,
                JOURNALS_RENDER_ID,
                JOURNALS_AUTO_CREATE_ID,
                JOURNALS_ACTION_ID,
            ] {
                assert!(
                    entry_ids.contains(id),
                    "fresh={fresh}: journals block {id} must be seeded"
                );
            }
            // Exactly ONE Journals page shell — never a duplicate.
            let page_count = entries
                .iter()
                .filter(|b| b.is_page() && b.content == "Journals")
                .count();
            assert_eq!(page_count, 1, "fresh={fresh}: exactly one Journals page");
        }
    }

    #[test]
    fn journals_disk_assets_are_empty_no_duplicate_page_source() {
        // The journals page is NOT a disk-seeded asset anymore (a disk Journals.org
        // with no `#+ID:` parsed to a SECOND `file:` page). Guard the regression.
        assert!(
            DEFAULT_ASSETS.is_empty(),
            "journals must be seeded programmatically, not written to disk"
        );
        // ContentType round-trips through the worker's string SQL path.
        assert_eq!(ContentType::Source.to_string(), "source");
    }
}
