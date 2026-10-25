//! @c4 component
//! @c4 layer Core
//! Pattern: MVVM ViewModel
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//! @c4 uses holon-advice "runtime advice rules" "Rust"
//! @c4 uses holon-oracles "live ref-less invariants" "Rust"
//! @c4 uses holon-org-format "org parse/render" "Rust"
//! @c4 uses holon-pbt-core "PBT transition traits" "Rust"
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
pub mod answer;
pub mod bridge_thread;
pub mod cdc;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod collection_layout;
pub mod dispatch_journal;
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

/// A default org file bundled with the app. Its `#+ID` and every block's `:ID:`
/// are fixed in the file, so seeding it again is a no-op per block: the seed
/// skips the blocks that exist and creates the missing ones.
pub struct DefaultAsset {
    pub filename: &'static str,
    pub content: &'static str,
}

impl DefaultAsset {
    /// The asset's page (from its `#+ID`) followed by its blocks, in document
    /// order.
    pub fn blocks(&self) -> Result<Vec<holon_api::block::Block>> {
        let parsed = holon_org_format::parse_org_file(
            std::path::Path::new(self.filename),
            self.content,
            &EntityUri::no_parent(),
            std::path::Path::new(""),
        )
        .with_context(|| format!("parse the bundled asset {}", self.filename))?;
        anyhow::ensure!(
            parsed.headlines_needing_ids.is_empty(),
            "bundled asset {} has headlines without an :ID: ({:?}); a re-seed would duplicate \
             them",
            self.filename,
            parsed.headlines_needing_ids
        );
        let mut blocks = vec![parsed.document];
        blocks.extend(parsed.blocks);
        Ok(blocks)
    }
}

/// The org files the seed writes into every vault that lacks them, and into
/// the store by id.
pub const DEFAULT_ASSETS: &[DefaultAsset] = &[DefaultAsset {
    filename: "Journals.org",
    content: include_str!("../../../assets/default/Journals.org"),
}];

pub mod command_provider;
pub mod config;
pub mod creation_slot;
pub mod echo;
pub mod editor_source;
pub mod editor_view_model;
pub mod expand_toggle;
pub mod focus_path;
pub mod geometry;
pub mod render_services;
pub use geometry::disclosure_halo_id_for;
pub use geometry::drawer_toggle_id_for;
pub use geometry::expand_toggle_id_for;
pub use geometry::tree_bullet_id_for;
pub use geometry::vms_button_id_for;
#[cfg(not(target_arch = "wasm32"))]
pub mod crash_history;
pub mod editor_caret;
pub mod headless_editor_mirror;
pub mod input;
pub mod input_trigger;
pub mod integration_vars;
pub(crate) mod link_provider;
pub mod link_segments;
pub mod live_block_ancestors;
pub mod local_edit_epoch;
pub mod logging;
pub mod memory_monitor;
pub mod mutable_tree;
pub mod navigation;
pub(crate) mod operation_matcher;
pub mod operations;
#[cfg(not(target_arch = "wasm32"))]
pub mod panic_record;
pub mod param_collection;
/// PBT SUT capability traits owned by the frontend (cap home-rule). Gated
/// behind the `pbt` feature so production builds never pull `holon-pbt-core`.
#[cfg(feature = "pbt")]
pub mod pbt_caps;
pub mod platform;
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
pub mod sticky_accordion;
pub mod template_placement;
pub mod theme;
pub mod theme_arg;
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

use anyhow::Context;
use anyhow::Result;
pub use config::HolonConfig;
pub use config::SessionConfig;
pub use config::UiConfig;
pub use config::WidgetState;
// Re-export types needed by consumers
pub use editor_view_model::{EditorAction, EditorKey, EditorViewModel};
use futures::future::BoxFuture;
use holon_api::EntityName;
use holon_api::EntityUri;
pub use holon_api::OperationDescriptor;
pub use holon_api::PathContext;
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
pub use reactive_view_model::NotAPropsUpdate;
pub use reactive_view_model::ReactiveSlot;
pub use reactive_view_model::ReactiveViewModel;
pub use reactive_view_model::extract_item_template;
pub use render_context::AvailableSpace;
pub use render_context::LayoutHint;
pub use render_context::RenderContext;
pub use row_origin::Occurrence;
pub use row_origin::OccurrenceId;
pub use row_origin::RowOrigin;
pub use shadow_builders::DEFAULT_DRAWER_WIDTH;
pub use shadow_builders::DRAWER_TOGGLE_WIDTH;
pub use user_driver::ReactiveEngineDriver;
pub use user_driver::StateToggleVerb;
pub use user_driver::UserDriver;
pub use view_model::ViewModel;

/// Unified session for all frontend consumers (Flutter, TUI, tests)
///
/// Ensures consistent initialization:
/// 1. Registers configured modules
/// 2. Waits for background tasks to be ready
/// 3. Tracks app errors (every bump site is a mid-run op failure)
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
    /// Answers "does this scheme name a registered entity?" against the LIVE
    /// registry. Marks persist only that a target is scheme-shaped, so link
    /// decoration and link-click ask this at read time — a link ingested
    /// before its provider connects heals on the next render.
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
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
    /// Where this session reads and writes credential preferences.
    ///
    /// A `OnceLock` rather than a constructor argument so a test can supply an
    /// in-memory store before the first render without every construction site
    /// learning about keychains — the same shape as
    /// `McpIntegrationsModule::with_browser`. Left unset it initialises to the
    /// OS keychain on first use.
    secret_store: std::sync::OnceLock<Arc<dyn holon_secrets::KeychainStore>>,
    /// Preference keys locked by CLI/env (read-only in UI).
    locked_keys: HashSet<preferences::PrefKey>,
    /// What this session's boot path skipped, and why.
    boot_report: platform::BootReport,
}

/// Whether this process refuses the machine's real login keychain.
///
/// Derived from the grant rather than latched separately: two independent
/// switches over one property is how they drift apart, and the grant is the
/// one every reader of a secret already answers to.
pub fn platform_keychain_forbidden() -> bool {
    !holon_secrets::login_keychain_granted()
}

/// Execute an intent as the user's gesture: an editor keystroke through the
/// engine's keystroke channel, anything else by its operation name.
pub fn execute_gesture(
    engine: &dyn holon_api::OperationEngine,
    intent: crate::operations::OperationIntent,
) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
    execute_gesture_parts(
        engine,
        intent.keystroke(),
        &intent.entity_name,
        &intent.op_name,
        intent.params,
    )
}

fn execute_gesture_parts(
    engine: &dyn holon_api::OperationEngine,
    keystroke: Option<holon_api::SourceKeystroke>,
    entity_name: &EntityName,
    op_name: &str,
    params: HashMap<String, Value>,
) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
    match keystroke {
        Some(keystroke) => engine.commit_keystroke(keystroke),
        None => engine.execute_operation(
            entity_name,
            op_name,
            params.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            holon_api::OpOrigin::User,
        ),
    }
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
    pub link_classifier: holon_api::link_parser::LinkTargetClassifier,
    pub error_tracker: PublishErrorTracker,
    pub ready_signal: Option<tokio::sync::watch::Receiver<Option<Result<(), String>>>>,
    pub preference_defs: Arc<Vec<preferences::PreferenceDef>>,
    pub theme_registry: Arc<theme::ThemeRegistry>,
    pub holon_config: config::HolonConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<preferences::PrefKey>,
    /// Proof that this boot path closed its capability ledger, plus what it
    /// skipped. Required (not optional) so a boot path cannot drop the
    /// disclosure without failing to compile — see
    /// [`crate::platform::BootDisclosure::finish`].
    pub boot_report: platform::BootReport,
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
        link_classifier: holon_api::link_parser::LinkTargetClassifier,
        boot_report: platform::BootReport,
    ) -> Self {
        // Note for the boot ledger: these three lines mean any caller of this
        // constructor HAS performed `ThemeAndPreferences` and
        // `PublishErrorTracker` — they should mark those steps performed.
        let theme_registry = Arc::new(theme::ThemeRegistry::load(None));
        let preference_defs = Arc::new(preferences::define_preferences(&theme_registry));
        Self {
            query_engine,
            block_query,
            operation_engine,
            ui_watcher,
            profiles,
            link_classifier,
            error_tracker: PublishErrorTracker::new(),
            ready_signal: None,
            preference_defs,
            theme_registry,
            holon_config: config::HolonConfig::default(),
            config_dir: PathBuf::new(),
            locked_keys: HashSet::new(),
            boot_report,
        }
    }
}

impl FrontendSession<()> {
    /// Construct a session from pre-assembled capabilities. The only public
    /// constructor — all backend assembly (Turso DI, no-Turso Loro stack)
    /// lives in the wiring crate and funnels through here.
    pub fn from_parts(parts: SessionParts) -> Self {
        // Derived here, not asked of the caller: the schema and the config
        // meet at exactly this point, and a wiring crate that forgot would
        // write credentials into plaintext with nothing to notice.
        // A schema defect, not user input: two secret keys sharing one keychain
        // account would make one integration authenticate with the other's
        // credential, and it would present as a wrong-token failure nobody
        // could trace back to a preference name. Loud at startup instead.
        preferences::assert_secret_accounts_are_distinct(&parts.preference_defs).unwrap_or_else(
            |e| panic!("[FrontendSession] the preference schema is unusable: {e:#}"),
        );
        let mut holon_config = parts.holon_config;
        holon_config.set_secret_keys(preferences::secret_keys(&parts.preference_defs));
        Self {
            query_engine: parts.query_engine,
            block_query: parts.block_query,
            operation_engine: parts.operation_engine,
            ui_watcher: parts.ui_watcher,
            profiles: parts.profiles,
            link_classifier: parts.link_classifier,
            error_tracker: parts.error_tracker,
            ready_signal: parts.ready_signal,
            extras: (),
            _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
            preference_defs: parts.preference_defs,
            theme_registry: parts.theme_registry,
            holon_config: Mutex::new(holon_config),
            config_dir: parts.config_dir,
            locked_keys: parts.locked_keys,
            secret_store: std::sync::OnceLock::new(),
            boot_report: parts.boot_report,
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

    /// What this session's boot path skipped, and why — the typed form of the
    /// startup capability warnings, for a frontend that wants to show them.
    pub fn boot_report(&self) -> &platform::BootReport {
        &self.boot_report
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

    /// The live scheme classifier — the read-time authority on whether a
    /// scheme-shaped link target names a registered entity.
    pub fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
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

    /// The config directory (where `holon.toml` and other app-local state
    /// live).
    pub fn config_dir(&self) -> &std::path::Path {
        &self.config_dir
    }

    /// Mutate UI config and persist to disk.
    ///
    /// The in-memory mutation always applies; a persistence failure (e.g. a
    /// read-only config dir) is surfaced loudly in the log rather than
    /// aborting the process. Widget-open toggles run through here — they must
    /// never SIGABRT the app on a failed disk write.
    pub fn update_ui_settings(&self, f: impl FnOnce(&mut UiConfig)) {
        let mut guard = self.holon_config.lock().unwrap();
        f(&mut guard.ui);
        if let Err(e) = guard.save_runtime(&self.config_dir) {
            tracing::error!(
                "[config] failed to persist UI settings to {}: {e:#}",
                self.config_dir.display()
            );
        }
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
    ///
    /// A PURE STORE WRITE — it invalidates nothing, because a `FrontendSession`
    /// holds no `UiState` (that lives one layer up, in `reactive`). Any caller
    /// on a frontend that renders from the SNAPSHOT must also invalidate, or
    /// `ViewKind::Drawer.open` — stamped at interpretation time — keeps its
    /// pre-toggle value: go through
    /// [`crate::reactive::BuilderServices::set_widget_open`], which bumps the
    /// viewport generation for you. GPUI calls this directly and is correct
    /// without it: it renders live off the view store and self-invalidates with
    /// its own `rebuild`/`cx.notify`, consuming no snapshot stream.
    pub fn set_widget_open(&self, block_id: &str, open: bool) {
        self.update_ui_settings(|s| {
            s.widgets.entry(block_id.to_string()).or_default().open = open;
        });
    }

    /// Set a widget's stored width.
    ///
    /// `persist == false` mutates the in-memory config only — used for the
    /// per-frame updates of a live drag-resize, where writing `holon.toml` on
    /// every mouse-move would hammer the disk. `persist == true` writes through
    /// to disk (call it once, on drag release) so the chosen width survives a
    /// restart.
    pub fn set_widget_width(&self, block_id: &str, width: f32, persist: bool) {
        if persist {
            self.update_ui_settings(|s| {
                s.widgets.entry(block_id.to_string()).or_default().width = Some(width);
            });
        } else {
            let mut guard = self.holon_config.lock().unwrap();
            guard
                .ui
                .widgets
                .entry(block_id.to_string())
                .or_default()
                .width = Some(width);
        }
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
    ///
    /// The in-memory mutation always applies. Returns `Err` (never panics) when
    /// the write can't be persisted so the caller can surface a visible
    /// degraded-mode notice — a preference that fails to save must keep the app
    /// alive, not abort it.
    pub fn set_preference(
        &self,
        key: &preferences::PrefKey,
        value: toml::Value,
    ) -> anyhow::Result<()> {
        let mut guard = self.holon_config.lock().unwrap();
        // A credential goes to the OS keychain. It is ALSO kept in the
        // in-memory map so this session behaves as before; `save_runtime`
        // strips it from what reaches `holon.toml`.
        if guard.is_secret_preference(key) {
            self.store_secret_preference(key, &value)?;
        }
        guard.set_preference(key, value);
        guard.save_runtime(&self.config_dir)
    }

    /// Read and write credential preferences through `store` instead of the OS
    /// keychain.
    ///
    /// For tests and for a profile that deliberately runs without one. Must be
    /// called before the first settings render or the first secret write;
    /// after that the OS keychain is already bound and this is a no-op, which
    /// it says out loud rather than silently pointing half the session at a
    /// different store.
    pub fn use_secret_store(
        &self,
        store: Arc<dyn holon_secrets::KeychainStore>,
    ) -> anyhow::Result<()> {
        self.secret_store.set(store).map_err(|_| {
            anyhow::anyhow!(
                "this session already resolved its secret store; call use_secret_store before the \
                 first settings render or secret write"
            )
        })
    }

    /// This session's secret store, binding the OS keychain on first use
    /// unless one was injected.
    ///
    /// Private: handing out the store would hand out `store`/`delete`, and
    /// every credential write belongs to [`Self::store_secret_preference`],
    /// which owns the empty-value-clears rule and the error wrapping. Tests
    /// seed through [`Self::seed_secret_for_test`] and read presence through
    /// [`Self::secret_is_stored`].
    fn secret_store(&self) -> &Arc<dyn holon_secrets::KeychainStore> {
        if let Some(store) = self.secret_store.get() {
            return store;
        }
        // Binding here would reach the machine's REAL login keychain. A test
        // that does so writes credentials into the developer's keychain and
        // leaves them there, silently. Only a production `main` grants the
        // keychain, so forgetting to inject is a loud stop everywhere else.
        assert!(
            !platform_keychain_forbidden(),
            "[FrontendSession] this process refuses the OS keychain, and this session had no \
             secret store injected. Call `FrontendSession::use_secret_store` before the first \
             settings render or secret write; a test fixture normally does it for you."
        );
        self.secret_store.get_or_init(|| {
            holon_secrets::platform_keychain(holon_secrets::INTEGRATION_SECRET_SERVICE).into()
        })
    }

    /// Put a secret preference's value into this session's secret store, under
    /// the account the `${VAR}` resolver reads.
    ///
    /// An empty value CLEARS the entry rather than storing a blank: a user who
    /// empties the field means "this is not configured", and an empty stored
    /// secret reads as unset everywhere else anyway.
    ///
    /// Fails loud. A secret the user believes is saved but that never reached
    /// the store is the silent-degradation failure this crate refuses, and the
    /// caller surfaces the error the way it surfaces a failed config write. No
    /// message here quotes the value.
    fn store_secret_preference(
        &self,
        key: &preferences::PrefKey,
        value: &toml::Value,
    ) -> anyhow::Result<()> {
        let account = integration_vars::normalize_var_name(key.as_str());
        let store = self.secret_store();
        let text = value.as_str().unwrap_or_default();
        if text.is_empty() {
            return store.delete(&account).map_err(|e| {
                anyhow::anyhow!("could not clear the stored secret for '{key}': {e:#}")
            });
        }
        store.store(&account, text.as_bytes()).map_err(|e| {
            anyhow::anyhow!(
                "could not save '{key}' to the keychain, so it is NOT stored: {e:#}. The value \
                 was not written to holon.toml either, because that file is plaintext."
            )
        })
    }

    /// Put a credential straight into this session's secret store, for a test
    /// FIXTURE that needs one present before the first render.
    ///
    /// Named for what it is. It bypasses [`Self::store_secret_preference`],
    /// and therefore the empty-value-clears rule and the error wrapping, which
    /// is exactly why production code must not reach for it.
    pub fn seed_secret_for_test(&self, account: &str, value: &[u8]) -> anyhow::Result<()> {
        self.secret_store()
            .store(account, value)
            .map_err(|e| anyhow::anyhow!("could not seed the secret store: {e:#}"))
    }

    /// Whether a credential is stored under `account`. Presence only — the
    /// value never leaves the store through this.
    ///
    /// A store that cannot be read is an `Err`, not a `false`: the caller is a
    /// fixture asserting on custody, and "no credential" versus "the store
    /// refused me" are the two outcomes it exists to tell apart.
    pub fn secret_is_stored(&self, account: &str) -> anyhow::Result<bool> {
        let found = self
            .secret_store()
            .load(account)
            .with_context(|| format!("read the secret store for account {account:?}"))?;
        Ok(found.is_some_and(|v| !v.is_empty()))
    }

    /// The secret preferences this profile actually holds a value for.
    ///
    /// A keychain read that FAILS is disclosed and counted as absent: the row
    /// then reads "Not set", which is the honest answer when the store cannot
    /// be read, and the warning says why. Claiming "stored" on a failed read
    /// would be the worse lie.
    fn stored_secret_keys(&self) -> HashSet<preferences::PrefKey> {
        let store = self.secret_store();
        preferences::secret_keys(&self.preference_defs)
            .into_iter()
            .filter(|key| {
                let account = integration_vars::normalize_var_name(key.as_str());
                match store.load(&account) {
                    Ok(found) => found.is_some_and(|v| !v.is_empty()),
                    Err(e) => {
                        tracing::warn!(
                            "[FrontendSession] could not read '{key}' from the keychain ({e:#}); \
                             its settings row will read as not set"
                        );
                        false
                    }
                }
            })
            .collect()
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
        // A field the environment shadows is read-only for the same reason a
        // CLI-sourced one is: what the user would type there is not what the
        // integration reads.
        let mut locked =
            preferences::env_shadowed_keys(&self.preference_defs, &|name| std::env::var(name).ok());
        locked.extend(self.locked_keys.iter().cloned());
        // Which credentials are actually held. A secret written to the
        // keychain is absent from `current`, so without this the row would
        // read "Not set" about a value that is in force.
        let stored_secrets = self.stored_secret_keys();
        let rows = preferences::preferences_to_rows(
            &self.preference_defs,
            &current,
            &locked,
            &stored_secrets,
        );
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

    /// Whether any publish or op-dispatch failure was recorded since boot.
    pub fn has_app_errors(&self) -> bool {
        self.error_tracker.has_errors()
    }

    /// Count of publish and op-dispatch failures recorded since boot.
    pub fn app_error_count(&self) -> usize {
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
            .is_none_or(|rx| rx.borrow().is_some())
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
    /// Build the default-layout seed blocks — the blocks of every
    /// [`DEFAULT_ASSETS`] file and the `__default__` page. Always returns
    /// these. Callers that want the full `index.org` layout must append the
    /// parsed org blocks separately (see `holon-app/src/seed.rs`).
    pub fn build_default_layout_blocks(fresh: bool) -> Result<Vec<holon_api::block::Block>> {
        use holon_api::block::Block;

        let default_doc_uri = Self::default_doc_uri();
        let mut entries: Vec<Block> = Vec::new();

        // Every bundled asset, on every boot: the ids are fixed in the asset,
        // so a block that exists is skipped and a missing one is created.
        for asset in crate::DEFAULT_ASSETS {
            entries.extend(asset.blocks()?);
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
        renderer: holon_api::render_requirements::RenderRequirements,
    ) -> Result<holon_api::EnrichedChangeStream> {
        self.query_engine()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "watch_query requires the Turso query engine, which is not wired in this \
                     (no-Turso) session"
                )
            })?
            .watch_query(query, language, params, context, renderer)
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
    ///
    /// Admits at the call (see [`holon_api::OperationEngine::admit`]), so a
    /// fire-and-forget caller calls this BEFORE it spawns the returned future.
    pub fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: HashMap<String, Value>,
    ) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
        self.execute_operation_with_origin(entity_name, op_name, params, holon_api::OpOrigin::User)
    }

    /// [`execute_gesture`] on this session's engine.
    pub fn execute_intent(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
        match self.require_operation_engine() {
            Ok(engine) => execute_gesture(engine.as_ref(), intent),
            Err(e) => Box::pin(std::future::ready(Err(e))),
        }
    }

    /// [`Self::execute_intent`] for a dispatch site that has already taken the
    /// intent apart: `keystroke` is what [`OperationIntent::keystroke`]
    /// returned for it.
    ///
    /// [`OperationIntent::keystroke`]: crate::operations::OperationIntent::keystroke
    pub fn execute_intent_parts(
        &self,
        keystroke: Option<holon_api::SourceKeystroke>,
        entity_name: &EntityName,
        op_name: &str,
        params: HashMap<String, Value>,
    ) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
        match self.require_operation_engine() {
            Ok(engine) => {
                execute_gesture_parts(engine.as_ref(), keystroke, entity_name, op_name, params)
            }
            Err(e) => Box::pin(std::future::ready(Err(e))),
        }
    }

    /// Execute an operation the session itself authored on the user's behalf
    /// rather than at their direct gesture — the creation-slot birth and its
    /// reaper (`OpOrigin::Rule`, ADR 0024's `fired-by` provenance slot). Such
    /// ops must NOT enter the human undo stack: the user did not author the
    /// empty block, so resurrecting it on Cmd-Z is noise.
    ///
    /// Everything else a `FrontendSession` dispatches is a direct user gesture
    /// and goes through [`Self::execute_operation`].
    pub fn execute_operation_with_origin(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: HashMap<String, Value>,
        origin: holon_api::OpOrigin,
    ) -> BoxFuture<'static, Result<holon_api::OpOutcome>> {
        match self.require_operation_engine() {
            Ok(engine) => engine.execute_operation(
                entity_name,
                op_name,
                params.into_iter().map(|(k, v)| (k.into(), v)).collect(),
                origin,
            ),
            Err(e) => Box::pin(std::future::ready(Err(e))),
        }
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
mod default_asset_tests {
    use super::*;

    fn ids(blocks: &[holon_api::block::Block]) -> Vec<String> {
        blocks.iter().map(|b| b.id.as_str().to_string()).collect()
    }

    #[test]
    fn every_asset_parses_to_one_root_page_with_fixed_ids() {
        for asset in DEFAULT_ASSETS {
            let blocks = asset.blocks().expect("parse asset");
            assert_eq!(
                ids(&blocks),
                ids(&asset.blocks().expect("parse asset again")),
                "{}: a re-seed must name the same ids",
                asset.filename
            );
            let page = &blocks[0];
            assert!(
                page.is_page(),
                "{}: the first block is its page",
                asset.filename
            );
            assert_eq!(page.parent_id, EntityUri::no_parent(), "{}", asset.filename);
            assert_eq!(
                blocks.iter().filter(|b| b.is_page()).count(),
                1,
                "{}: one page per asset",
                asset.filename
            );
        }
    }

    #[test]
    fn default_layout_carries_every_asset_block_on_every_boot() {
        let wanted: Vec<String> = DEFAULT_ASSETS
            .iter()
            .flat_map(|a| ids(&a.blocks().expect("parse asset")))
            .collect();
        for fresh in [true, false] {
            let entries =
                FrontendSession::<()>::build_default_layout_blocks(fresh).expect("build layout");
            let have = ids(&entries);
            for id in &wanted {
                assert!(
                    have.contains(id),
                    "fresh={fresh}: asset block {id} not seeded"
                );
            }
        }
    }
}
