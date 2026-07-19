//! Reactive middle layer using futures-signals.
//!
//! Replaces CdcAccumulator + BlockWatchRegistry + AppState with a single
//! reactive cache. Each watched block or live query gets a
//! `ReactiveRenderedRows` that IS the cache, the accumulator, AND the signal
//! source.
//!
//! ```text
//! Turso IVM → UiEvent → ReactiveRenderedRows → Signal<ViewModel> → Stream → Frontend
//!                        (IS the cache)         (IS the join)       (IS the API)
//! ```

use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use futures_signals::map_ref;
use futures_signals::signal::Mutable;
use futures_signals::signal::ReadOnlyMutable;
use futures_signals::signal::Signal;
use futures_signals::signal::SignalExt;
use futures_signals::signal_map::MutableBTreeMap;
use futures_signals::signal_vec::SignalVec;
use futures_signals::signal_vec::SignalVecExt;
use holon_api::EntityUri;
use holon_api::NavigationOp;
use holon_api::QueryLanguage;
use holon_api::ReactiveRowProvider;
use holon_api::ptr_identity;
use holon_api::render_types::RenderExpr;
use holon_api::streaming::UiEvent;
use holon_api::widget_spec::DataRow;
use holon_api::widget_spec::EnrichedRow;

use crate::FrontendSession;
use crate::WidgetState;
use crate::reactive_view_model::ReactiveViewModel;
use crate::render_context::RenderContext;
use crate::render_interpreter::RenderInterpreter;
use crate::view_model::ViewModel;

// ── BuilderServices trait ───────────────────────────────────────────────

/// Narrow capabilities available to builders during interpretation.
///
/// `ReactiveEngine` implements this. Builders never see `FrontendSession`
/// or `ReactiveEngine` directly — they call these methods through
/// `ctx.services` (an `Arc<dyn BuilderServices>`).
///
/// On the "headless / stub" defaults below: the E2E PBTs do NOT use a
/// stub. They run windowless (no GPUI surface) but drive the real
/// `ReactiveEngine`, which overrides every method here — focus tracking,
/// provider cache, viewport, keybindings, dispatch. The DI container
/// resolves `ReactiveEngine` and registers it as the `BuilderServices`
/// (see `test_environment.rs`), so PBT fidelity is intact. The
/// stub-returning defaults exist only for the two genuinely
/// non-interactive impls: holon-app's `HeadlessBuilderServices` (the MCP
/// `describe_ui` path) and `StubBuilderServices` (gpui layout/gallery tests).
/// "headless" in the per-method docs is a misnomer for "these two stub impls" —
/// it does not mean the windowless PBT harness.
pub trait BuilderServices: Send + Sync {
    /// Interpret `expr` against `ctx` using this services' shadow interpreter.
    ///
    /// The implementation passes `self` as `&dyn BuilderServices` to the
    /// interpreter so recursive builder calls stay inside the same engine.
    /// This is the one and only entry point for row interpretation in the
    /// reactive pipeline — no caller ever touches `RenderInterpreter` directly.
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel;

    /// Return an owned handle to this services instance.
    ///
    /// Needed by widgets that capture services for deferred interpretation
    /// (lazy slots, suspendable subscriptions). Implementors that participate
    /// in lazy-materialisation paths override this; the default panics so
    /// non-participating stubs (test, headless) fail loud if accidentally
    /// driven through such a path.
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        unimplemented!(
            "clone_arc not implemented for this BuilderServices impl; only services that drive \
             lazy widgets (expand_toggle, tabs, view_mode_switcher) need to override this"
        )
    }

    /// Get the current (RenderExpr, Vec<Arc<DataRow>>) for a block, ensuring a
    /// watcher is running.
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>);

    /// Resolve the entity profile for a data row. Returns `None` when no entity
    /// type could be inferred.
    fn resolve_profile(&self, row: &DataRow) -> Option<holon_api::RenderProfile>;

    /// Mutable holding the current profile registry snapshot.
    ///
    /// Each rebuild swaps in a fresh `Arc<ProfileCache>`, firing the signal.
    /// `render_entity` reads the current profile inside `interpret`, but
    /// `interpret_row` only re-runs on per-row data changes — so a
    /// profile-only edit otherwise leaves already-rendered items frozen at
    /// the pre-mutation profile. Collection drivers subscribe to this
    /// signal and trigger a full re-interpret when it fires.
    ///
    /// Default: an empty cache that never changes (for stub/headless services).
    fn profile_signal(&self) -> Mutable<Arc<holon_api::entity_profile::ProfileCache>> {
        Mutable::new(Arc::new(holon_api::entity_profile::ProfileCache::empty()))
    }

    /// Get the virtual child config for an entity type, if declared in its
    /// profile.
    fn virtual_child_config(
        &self,
        _: &str,
    ) -> Option<holon_api::entity_profile::VirtualChildConfig> {
        None
    }

    /// Advice rows to weave as read-only children under `anchor` (ADR 0022),
    /// already synthesized (rank-ordered `DataRow`s, `parent_id = anchor`,
    /// occurrence-keyed columns). Empty when no active rule matches / not yet
    /// computed. A SYNCHRONOUS pure read of the pre-populated session sidecar —
    /// mirrors [`Self::virtual_child_config`]; every
    /// stub/headless-without-advice service keeps the default (empty), so
    /// snapshots stay byte-identical.
    // ALLOW(unused_param): trait default; overriding impls bind `anchor`
    fn advice_children(&self, _: &EntityUri) -> Vec<Arc<DataRow>> {
        Vec::new()
    }

    /// Entity-level operations (keyed by id scheme, e.g. `"block"`) — the same
    /// set the renderer attaches to a row of that entity. Used by headless
    /// input paths to build the slash-command menu without a rendered node.
    /// Default: empty (stub/headless services without a resolver).
    fn entity_operations(&self, _: &str) -> Vec<holon_api::render_types::OperationDescriptor> {
        Vec::new()
    }

    /// Compile a query (PRQL/GQL/SQL) and start a live CDC watch, returning
    /// the enriched change stream. SQL compilation happens *behind* this
    /// capability — builders never see SQL strings or the raw Turso stream.
    /// Blocks until the stream is established.
    fn watch_query(
        &self,
        query: &str,
        lang: QueryLanguage,
        ctx: Option<crate::QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream>;

    /// The one-shot query-execution capability, when a Turso query engine is
    /// wired. `None` for a no-Turso (Loro-only) session. The advice weaver
    /// reaches it via this accessor to run its canonical read as a single
    /// [`holon_api::QueryEngine::execute_query`] — NEVER a watch (see
    /// `crate::advice_weaver`). Default `None` (stub/headless services).
    fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        None
    }

    /// Look up widget state by block ID.
    fn widget_state(&self, id: &str) -> WidgetState;

    /// View-local expansion seed for an `expand_toggle` whose `target_id` is
    /// `target_id`: `Some(expanded)` when the user has driven this toggle,
    /// `None` (default) to keep the collapsed-until-clicked default. The
    /// engine-backed impl records the read for fail-loud driver checks. This is
    /// the only mechanism that survives a fresh `snapshot()` for profile-driven
    /// embedded pages (no `collapsed` field). Default `None` keeps every
    /// non-engine `BuilderServices` behaviour-identical.
    fn block_expanded_view(&self, target_id: &str) -> Option<bool> {
        let _ = target_id;
        None
    }

    /// Look up the *explicitly stored* widget state, or `None` when the user
    /// never toggled it. The default treats every widget as explicit (i.e.
    /// preserves the legacy open-by-default semantics); the real session-backed
    /// impl overrides it so [`Self::drawer_open`] can default overlay drawers
    /// closed.
    fn widget_state_explicit(&self, id: &str) -> Option<WidgetState> {
        Some(self.widget_state(id))
    }

    /// Effective open state for a drawer in the given mode.
    ///
    /// An explicit user setting always wins. With no stored state, the default
    /// is mode-dependent: `Shrink` drawers (desktop sidebars that reserve
    /// width) start open, while `Overlay` drawers (narrow/phone layouts
    /// that float over the main panel) start closed — an open-by-default
    /// overlay would obscure the content on first paint.
    fn drawer_open(&self, id: &str, mode: crate::view_model::DrawerMode) -> bool {
        self.widget_state_explicit(id)
            .map(|s| s.open)
            .unwrap_or_else(|| mode.default_open())
    }

    /// Set a widget's `open` field. Used by self-rendering toggle widgets
    /// (`drawer`, `collapse_toggle`) so the click handler doesn't have to
    /// reach into `FrontendSession` directly. Default impl panics — every
    /// real `BuilderServices` impl must override; the stub providers
    /// (`StubBuilderServices`, ref-state mock) override it as a no-op.
    fn set_widget_open(&self, id: &str, open: bool) {
        let _ = (id, open);
        unimplemented!("BuilderServices::set_widget_open");
    }

    /// Fire-and-forget operation dispatch.
    ///
    /// Spawns the operation on the runtime and logs errors. This replaces the
    /// pattern of downcasting to `ReactiveEngine` just to get `session()` +
    /// `runtime_handle()` and calling `dispatch_operation()` manually.
    fn dispatch_intent(&self, intent: crate::operations::OperationIntent);

    /// Persist a single preference synchronously, returning `Err` on a write
    /// failure so the caller (e.g. a GPUI preference field) can surface a
    /// visible degraded-mode toast instead of the process aborting. Preference
    /// writes run inline (blocking file IO) rather than through the async
    /// `dispatch_intent` op path. Default no-ops — stub/headless services do
    /// not persist config.
    fn set_preference(&self, key: &str, value: holon_api::Value) -> Result<()> {
        let _ = (key, value);
        Ok(())
    }

    /// Synchronous operation dispatch — awaits completion and returns the
    /// operation's result.
    ///
    /// Tests, MCP tool handlers, and `ReactiveEngineDriver` want to know when
    /// the operation has actually taken effect (so they can read back state);
    /// they should call this instead of the fire-and-forget `dispatch_intent`.
    /// GPUI click handlers still use `dispatch_intent` because they must not
    /// block the UI thread.
    ///
    /// Default impl delegates to `dispatch_intent` without waiting — only
    /// stub/headless services where the operation has no observable effect
    /// should fall back to the default.
    fn dispatch_intent_sync(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        self.dispatch_intent(intent);
        Box::pin(std::future::ready(Ok(())))
    }

    /// Like [`Self::dispatch_intent`] but returns a `'static` future resolving
    /// to the op's result, so a caller (e.g. the GPUI editor) can await it and
    /// surface a BACKEND failure — template-not-found, missing bindings — as a
    /// visible toast instead of only a log line. Default fire-and-forget + `Ok`
    /// (stub/headless has no backend result to await).
    fn dispatch_intent_awaitable(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'static>> {
        self.dispatch_intent(intent);
        Box::pin(std::future::ready(Ok(())))
    }

    /// Follow a click on a *dangling* wiki-link — a `[[Name]]` mark whose
    /// target does not yet resolve to a block. Creates the page chain for
    /// `target` (via the `create_page_from_link` op), then navigates `region`
    /// to the freshly-created leaf page, so the click feels identical to
    /// clicking an already-resolved link (which dispatches `navigation.focus`).
    ///
    /// The leaf page id is a fresh UUID known only from the create op's
    /// response, so the create→navigate step must chain in-process — it cannot
    /// be expressed as two independent fire-and-forget intents. This default
    /// impl fires only the create (stub/headless services have no navigable
    /// `main` region to focus); `ReactiveEngine` overrides it to also navigate.
    fn follow_dangling_link(&self, target: String, region: String) {
        let _ = region;
        self.dispatch_intent(crate::operations::OperationIntent::new(
            holon_api::EntityName::new("block"),
            "create_page_from_link".into(),
            [("target".to_string(), holon_api::Value::String(target))]
                .into_iter()
                .collect(),
        ));
    }

    /// Present an operation at the op_button tap site.
    ///
    /// Routing contract implementers must provide:
    /// - If all of `op.required_params` are satisfied by `ctx_params`
    ///   (typically just `id`): build an `OperationIntent` and dispatch
    ///   immediately.
    /// - If more params are needed: activate a popup param-collection flow
    ///   (same machinery `CommandProvider` drives for the slash menu).
    ///
    /// No default impl — every `BuilderServices` implementer must declare
    /// routing explicitly. Headless / Stub impls **panic** (an `op_button`
    /// should never be interpreted under a non-interactive services
    /// instance, because its YAML branch is gated on `if_space(<600)` in
    /// an interactive session).
    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        ctx_params: HashMap<String, holon_api::Value>,
    );

    /// Get the current UI state for predicate evaluation.
    ///
    /// Returns context variables like `is_focused`, `view_mode` that
    /// `Predicate::evaluate()` can use to pick the active render variant.
    /// Default: empty map (all UI predicates evaluate to false/default).
    fn ui_state(&self, _: &EntityUri) -> HashMap<String, holon_api::Value> {
        HashMap::new()
    }

    /// Current root viewport allocation as an `AvailableSpace`, if known.
    ///
    /// Used by `interpret_pure` / `snapshot_resolved` to seed the root
    /// `RenderContext.available_space` so that top-level `if_space(...)`
    /// and profile-variant `available_*` predicates evaluate against the
    /// live window size. Returns `None` before the platform shell has
    /// pushed an initial viewport.
    fn viewport_snapshot(&self) -> Option<crate::render_context::AvailableSpace> {
        None
    }

    /// Snapshot of the current keybinding registry: operation_name → key chord.
    /// Used by RenderContext::with_operations() to join keybindings into
    /// operations.
    fn key_bindings_snapshot(&self) -> std::collections::BTreeMap<String, holon_api::KeyChord> {
        std::collections::BTreeMap::new()
    }

    // ── UI state (focus, view mode) ─────────────────────────────────────

    /// Get the currently focused block ID.
    fn focused_block(&self) -> Option<EntityUri> {
        None
    }

    /// Cloned handle to the focused-block `Mutable`, when this services
    /// instance is backed by a `UiState`. Used by reactive row providers
    /// like `focus_chain` that need a long-lived signal source rather than
    /// a one-shot snapshot.
    ///
    /// `ReactiveEngine` returns `Some` — so the E2E PBTs (which use the real
    /// engine) have full focus tracking. The outer `Option` is NOT
    /// vestigial: it returns `None` for the two non-interactive stub impls
    /// (holon-app's `HeadlessBuilderServices` for MCP, `StubBuilderServices`
    /// for gpui gallery tests), where there is no UI focus to track. The
    /// three callers (`focus_chain`, `chain_ops`, `reactive_view`)
    /// deliberately degrade to empty/suspended on `None` rather than
    /// fabricate a focus.
    fn focused_block_mutable(&self) -> Option<Mutable<Option<EntityUri>>> {
        None
    }

    /// Shared provider cache for reactive value-fn row providers
    /// (`focus_chain`, `ops_of`, ...). Returns `None` for headless /
    /// stub services that don't own a cache; callers must fall back to
    /// constructing providers directly. `ReactiveEngine` returns its
    /// own `provider_cache`.
    fn provider_cache(&self) -> Option<Arc<crate::provider_cache::ProviderCache>> {
        None
    }

    /// Set the currently focused block. Pass `None` to clear focus.
    fn set_focus(&self, _: Option<EntityUri>) {}

    /// Focus `block` and arm a one-shot initial caret offset for the editor
    /// that mounts for it (split → 0, join → boundary, cross-block nav →
    /// placement). The mounting editor reads it via [`peek_caret_seed`]. The
    /// default delegates to `set_focus` (dropping the offset) for services
    /// with no caret to seed.
    fn set_focus_with_caret(&self, block: EntityUri, _: usize) {
        self.set_focus(Some(block));
    }

    /// Read (without consuming) the one-shot initial caret offset armed for
    /// `block` by a split/join/nav focus move. Returns the offset, or `None` —
    /// in which case the mounting editor defaults the caret to end-of-text.
    /// Headless/stub services have no caret to seed.
    fn peek_caret_seed(&self, _: &EntityUri) -> Option<usize> {
        None
    }

    /// Consume (clear) the caret seed armed for `block`, once the mounting
    /// editor has applied it. The seed is a one-shot op-follow-up placement
    /// (split → 0, join → boundary, nav → offset); leaving it armed lets a
    /// LATER user click on the same block re-apply the stale offset, yanking
    /// the caret away from where the click landed (caret-0 → prepend
    /// corruption after split+join, BugFunnel 2026-07-11 row 80). Consuming on
    /// application makes it strictly single-use: a click always derives the
    /// caret from the current buffer. No-op when the armed seed targets a
    /// different block. Headless/stub services have no seed to consume.
    fn consume_caret_seed(&self, _: &EntityUri) {}

    /// Get a [`Cell<String>`] handle for collaborative editing of a block
    /// field. Resolves through the `BlockCellRegistry` (Loro-backed when
    /// LoroModule is loaded). Returns `Err` for headless/stub services
    /// that don't have a LoroDoc, or for blocks not yet present in the
    /// Loro tree.
    fn editable_text(&self, _: &EntityUri, _: &str) -> anyhow::Result<crate::cell::Cell<String>> {
        Err(anyhow::anyhow!(
            "editable_text not supported by this BuilderServices implementation"
        ))
    }

    /// Fully-resolved static snapshot of a block's UI tree.
    ///
    /// Interprets the block's render expression against its current data rows,
    /// then recursively resolves every nested `LiveBlock` placeholder by
    /// calling itself for each embedded block. Returns a `ViewModel`
    /// suitable for serialization (MCP `describe_ui`, PBT assertions, TUI
    /// rendering).
    ///
    /// Default implementation composes `get_block_data` +
    /// `interpret_with_source`
    /// + `snapshot_resolved`. Implementors with an optimized watcher path (e.g.
    /// `ReactiveEngine::ensure_watching`) can override.
    fn snapshot_resolved(&self, block_id: &EntityUri) -> crate::view_model::ViewModel {
        let (expr, rows) = self.get_block_data(block_id);
        let ctx = RenderContext {
            data_rows: rows.into(),
            available_space: self.viewport_snapshot(),
            ..Default::default()
        };
        let rvm = self.interpret(&expr, &ctx);
        rvm.snapshot_resolved(&|bid| self.snapshot_resolved(bid))
    }

    /// Wait until the first Structure event has been received for a block.
    /// Returns immediately if the block's render expression is already loaded.
    /// Default: returns a ready future (for headless/stub impls that don't
    /// stream).
    fn await_ready(
        &self,
        _: &EntityUri,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }

    /// Get a reactive signal for a block's UI. Each call returns an independent
    /// signal that tracks the block's render expression + data.
    fn watch_block_signal(
        &self,
        _: &EntityUri,
    ) -> std::pin::Pin<
        Box<dyn futures_signals::signal::Signal<Item = crate::ReactiveViewModel> + Send>,
    > {
        panic!("watch_block_signal not supported by this BuilderServices implementation")
    }

    /// Watch a block with per-row collection reactivity.
    ///
    /// Returns a `LiveBlock` whose tree has ReactiveView nodes that self-manage
    /// their streaming pipelines. `structural_changes` fires only on render
    /// expression changes.
    fn watch_live(&self, _: &EntityUri, _: Arc<dyn BuilderServices>) -> crate::LiveBlock {
        panic!("watch_live not supported by this BuilderServices implementation")
    }

    /// Stop watching a block and release its reactive state (watchers,
    /// MutableVec items). No-op by default. Implemented by ReactiveEngine.
    fn unwatch(&self, _: &EntityUri) {}

    /// Watch a live query with per-row collection reactivity.
    ///
    /// Returns the engine's watcher key (pass it to [`Self::unwatch`] to
    /// release the query watcher) and a `LiveBlock` whose tree has
    /// ReactiveView nodes that self-manage their streaming pipelines.
    /// `structural_changes` fires only on render-expression / ui-generation
    /// changes — data-only changes update the tree in place. Takes the
    /// source query + language; compilation happens behind the query
    /// capability.
    fn watch_query_live(
        &self,
        _: String,
        _: QueryLanguage,
        _: holon_api::render_types::RenderExpr,
        _: Option<crate::QueryContext>,
        _: Arc<dyn BuilderServices>,
    ) -> (EntityUri, crate::LiveBlock) {
        panic!("watch_query_live not supported by this BuilderServices implementation")
    }

    /// Tokio runtime handle for spawning subscriptions (editor/popup providers,
    /// reactive watchers). Replaces the side-channel `rt_handle` field that
    /// used to live on `GpuiRenderContext`. Impls without a runtime must still
    /// panic loudly (fail loud per CLAUDE.md) — never return a dummy handle.
    fn runtime_handle(&self) -> tokio::runtime::Handle;

    /// Optional runtime handle — `Some` for live frontends with a tokio
    /// runtime, `None` for sync-only contexts (PBT reference model, shadow
    /// interpretation). Builders that spawn signal subscriptions to derive
    /// reactive props (e.g. `state_toggle`'s `data` → `current`/`label`
    /// derivation) should consult this *first* and skip the subscription
    /// when no runtime is available — those call sites build a snapshot
    /// once and don't need live updates.
    fn try_runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        Some(self.runtime_handle())
    }

    /// Enumerate the vault's templates (blocks carrying the `template`
    /// property) for the slash-command picker. Default empty — a stub/headless
    /// service without a block projection offers no templates. The engine impl
    /// reads the block snapshot; the result feeds `CommandProvider`'s
    /// per-template entries.
    fn list_templates(&self) -> Vec<crate::template_placement::TemplateChoice> {
        Vec::new()
    }

    /// Resolve a single block's fields (content, parent) by id, out of the
    /// block projection — the picker's placement decision needs the REAL block,
    /// not the editor's id-only `context_params`. Default `None` (stub/headless
    /// service without a projection). The engine impl reads the snapshot.
    fn resolve_block(&self, _: &str) -> Option<holon_api::block::Block> {
        None
    }

    /// Search entities matching `filter` for the `[[` link-autocomplete
    /// popup. Minimal async capability that replaces the
    /// `Arc<FrontendSession>` plumb line through the editor. The search SQL
    /// lives behind the query capability; headless/stub impls without a
    /// backend return `Err`.
    fn search_link_candidates(
        &self,
        filter: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    >;
}

// ── ReactiveRowSet ──────────────────────────────────────────────────────

/// Reactive accumulator for CDC row changes.
///
/// Accumulates `Change<DataRow>` diffs into a
/// `MutableBTreeMap<String, Mutable<Arc<DataRow>>>`, keyed by entity ID.
///
/// Per-row storage is `Mutable<Arc<DataRow>>` rather than `Arc<DataRow>` so
/// that field updates don't change the *entry identity* in the outer map.
/// `Updated` and `FieldsChanged` look up the existing cell and call `.set()`,
/// which fires the per-row signal but emits **no** outer `MapDiff`. Subscribers
/// that want per-row updates clone the cell (see `row_mutable`) and subscribe
/// to its signal directly. Subscribers that still want "full set as a Vec on
/// any change" use `data_signal()` / `row_signal_vec()` — those flatten the
/// inner Mutables transparently.
///
/// This is what makes the GPUI render path single-writer: the only writer is
/// `apply_change`; downstream nodes share the same `Arc<MutableState>` via
/// cloned `Mutable` handles. The convention "only `ReactiveRowSet` writes" is
/// not enforced by the type system — code review keeps it.
///
/// Used by `ReactiveRenderedRows` (which adds a RenderExpr for interpretation)
/// and directly by raw CDC watchers.
/// TODO: Please check if `generation` is actually needed. The `MutableBTreeMap`
/// should afaik only trigger if the data really changed. Run a small experiment
/// if unsure.
pub struct ReactiveRowSet {
    data: MutableBTreeMap<EntityUri, Mutable<Arc<DataRow>>>,
    generation: Mutable<u64>,
}

impl ReactiveRowSet {
    pub fn new() -> Self {
        Self {
            data: MutableBTreeMap::new(),
            generation: Mutable::new(0),
        }
    }

    /// Set the generation (invalidation token). Stale changes are discarded.
    pub fn set_generation(&self, generation: u64) {
        self.generation.set(generation);
    }

    /// Current generation.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Apply a single enriched row-level CDC change. Ignores stale generations.
    ///
    /// Accepts `Change<EnrichedRow>` — the caller must have gone through
    /// `enrich_row()` / `enrich_stream()` to obtain enriched data.
    /// This prevents accidentally feeding raw storage data into the reactive
    /// pipeline.
    pub fn apply_change(&self, change: holon_api::Change<EnrichedRow>, generation: u64) {
        if generation != self.generation.get() {
            return;
        }
        match change {
            holon_api::Change::Created { data, .. } => {
                let row = Arc::new(data.into_inner());
                // A Created row is either entity-shaped (real `id`) or
                // value-shaped (aggregate / rule-trigger result / future table
                // row — no `id`). Both are legal display cases: an id-less row
                // is keyed on its deterministic content hash, NOT panicked on
                // (a `.expect("... 'id' column")` here blanked the whole page
                // by killing the render worker — dogfood 2026-07-10).
                let key = holon_api::RowIdentity::of_row(&row).to_store_key();
                let mut lock = self.data.lock_mut();
                if let Some(existing) = lock.get(&key) {
                    // Defensive: a Created arriving for a row we already know
                    // about — treat as Updated to avoid losing the cell identity.
                    existing.set_neq(row);
                } else {
                    lock.insert_cloned(key, Mutable::new(row));
                }
            }
            holon_api::Change::Updated { id, data, .. } => {
                let row = Arc::new(data.into_inner());
                let key = holon_api::entity_uri_from_id_str(&id);
                let mut lock = self.data.lock_mut();
                if let Some(existing) = lock.get(&key) {
                    // set_neq: CDC echoes of locally-applied writes arrive with
                    // identical content — suppress them here so they don't fan
                    // out into per-row widget re-interpretation downstream.
                    existing.set_neq(row);
                } else {
                    // Out-of-order: Updated before Created. Insert so we don't
                    // drop the row.
                    lock.insert_cloned(key, Mutable::new(row));
                }
            }
            holon_api::Change::Deleted { id, .. } => {
                self.data
                    .lock_mut()
                    .remove(&holon_api::entity_uri_from_id_str(&id));
            }
            holon_api::Change::FieldsChanged {
                entity_id, fields, ..
            } => {
                let key = holon_api::entity_uri_from_id_str(&entity_id);
                let lock = self.data.lock_ref();
                if let Some(existing) = lock.get(&key) {
                    let mut patched = existing.get_cloned();
                    let map = Arc::make_mut(&mut patched);
                    for (name, _old, new) in fields {
                        map.insert(name, new);
                    }
                    existing.set_neq(patched);
                }
            }
        }
    }

    /// Drop every row whose key is not in `keys`. Used when a re-render's
    /// initial snapshot batch arrives: the snapshot is authoritative for the
    /// (possibly changed) query, so rows it doesn't contain are stale.
    pub fn retain_keys(&self, keys: &[EntityUri]) {
        let stale: Vec<EntityUri> = {
            let lock = self.data.lock_ref();
            lock.keys().filter(|k| !keys.contains(k)).cloned().collect()
        };
        if !stale.is_empty() {
            let mut lock = self.data.lock_mut();
            for key in stale {
                lock.remove(&key);
            }
        }
    }

    /// Number of rows, without materializing them.
    pub fn len(&self) -> usize {
        self.data.lock_ref().len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.lock_ref().is_empty()
    }

    /// Synchronous snapshot of current rows (Arc-wrapped, cheap to clone).
    pub fn snapshot_rows(&self) -> Vec<Arc<DataRow>> {
        self.data
            .lock_ref()
            .iter()
            .map(|(_, cell)| cell.get_cloned())
            .collect()
    }

    /// Get a shared `ReadOnlyMutable` handle to the per-row cell for `id`.
    ///
    /// The cell's writable `Mutable` lives only inside `self.data` —
    /// `apply_change` is the only writer. Consumers receive a read-only
    /// clone of the same `Arc<MutableState>`, so they observe every CDC
    /// update via signal subscription, but the type system makes leaf-side
    /// `.set()` impossible (no method exists on `ReadOnlyMutable`).
    /// Returns `None` if the row hasn't been seen yet.
    pub fn row_mutable(&self, id: &EntityUri) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
        self.data.lock_ref().get(id).map(|m| m.read_only())
    }

    /// Per-row `SignalVec`. Each item is the **current value** of an
    /// `Arc<DataRow>` cell. Flattens the per-row `Mutable` so the SignalVec
    /// emits `UpdateAt` on per-row writes (preserving the previous external
    /// contract where data updates surface as `VecDiff::UpdateAt`).
    pub fn row_signal_vec(&self) -> impl SignalVec<Item = Arc<DataRow>> {
        self.data
            .entries_cloned()
            .map_signal(|(_, cell)| cell.signal_cloned())
    }

    /// Signal that fires the full row set whenever any row changes.
    pub fn data_signal(&self) -> impl Signal<Item = Vec<(EntityUri, Arc<DataRow>)>> {
        self.data
            .entries_cloned()
            .map_signal(|(k, cell)| cell.signal_cloned().map(move |v| (k.clone(), v)))
            .to_signal_cloned()
    }

    /// Per-row `SignalVec` with keys. Each item is `(entity_id, Arc<DataRow>)`.
    ///
    /// Used by `MutableTree` to translate keyed VecDiff into tree operations.
    /// Unlike `row_signal_vec()`, preserves the entity ID for `RemoveAt`
    /// tracking.
    pub fn keyed_signal_vec(&self) -> impl SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> {
        // The store is `EntityUri`-keyed; every canonical row carries
        // `Occurrence::Canonical`. Display-placed occurrences are injected by
        // `AppendedRowsProvider`, never by the store.
        self.data.entries_cloned().map_signal(|(k, cell)| {
            cell.signal_cloned()
                .map(move |v| ((k.clone(), holon_api::Occurrence::Canonical), v))
        })
    }
}

// ── ReactiveRowProvider impls ────────────────────────────────────────────
//
// Exposes `ReactiveRowSet` and `ReactiveRenderedRows` through the trait
// object that streaming-collection widgets consume. Synthetic providers
// (focus_chain, ops_of, chain_ops — added in Step 8) implement the trait
// directly without backing an engine query.

impl ReactiveRowProvider for ReactiveRowSet {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        self.snapshot_rows()
    }
    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        Box::pin(self.row_signal_vec())
    }
    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> + Send>> {
        Box::pin(self.keyed_signal_vec())
    }
    fn cache_identity(&self) -> u64 {
        ptr_identity(self)
    }
    fn row_mutable(&self, id: &EntityUri) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
        self.row_mutable(id)
    }
}

// ── ReactiveRenderedRows ─────────────────────────────────────────────────

/// Reactive state for one query's result set + how to render it.
///
/// Composes a `ReactiveRowSet` (data accumulation) with a `RenderExpr`
/// (how to visualize it). Signals combine both into
/// `Signal<ReactiveViewModel>`.
pub struct ReactiveRenderedRows {
    render_expr: Mutable<RenderExpr>,
    rows: ReactiveRowSet,
    structure_ready: tokio::sync::Notify,
    /// Generation whose data currently populates `rows`. Each re-render
    /// (generation bump) starts a fresh data stream whose FIRST batch is a
    /// full authoritative snapshot (`prepend_initial_data`) — when it
    /// arrives we drop rows the new query no longer returns (see
    /// `apply_event`). Without this, a structural change that alters the
    /// underlying query (e.g. an org-file swap replacing the layout) leaves
    /// ghost rows from the old query in the set forever.
    data_generation: std::sync::atomic::AtomicU64,
}

impl ReactiveRenderedRows {
    pub fn new() -> Self {
        Self {
            render_expr: Mutable::new(loading_expr()),
            rows: ReactiveRowSet::new(),
            structure_ready: tokio::sync::Notify::new(),
            data_generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Set the render expression directly.
    pub fn set_render_expr(&self, expr: RenderExpr) {
        self.render_expr.set(expr);
    }

    /// Get the current render expression.
    pub fn get_render_expr(&self) -> RenderExpr {
        self.render_expr.get_cloned()
    }

    /// True if the first Structure event hasn't arrived yet.
    pub fn is_loading(&self) -> bool {
        matches!(
            &*self.render_expr.lock_ref(),
            RenderExpr::FunctionCall { name, .. } if name == "loading"
        )
    }

    /// Wait until the first Structure event delivers a real render expression.
    pub async fn wait_until_ready(&self) {
        if !self.is_loading() {
            return;
        }
        self.structure_ready.notified().await;
    }

    /// Set the generation (invalidation token) on the inner row set.
    pub fn set_generation(&self, generation: u64) {
        self.rows.set_generation(generation);
    }

    /// Apply a UiEvent directly. Single entry point for all CDC events.
    ///
    /// Structure → sets render_expr + generation. Does NOT clear data — the new
    /// data stream will overwrite when it arrives, avoiding flash of empty
    /// content. Data → row-level diffs into the row set. Stale generations
    /// discarded.
    pub fn apply_event(&self, event: UiEvent) {
        match event {
            UiEvent::Structure {
                render_expr,
                candidates: _,
                generation,
            } => {
                self.rows.set_generation(generation);
                if *self.render_expr.lock_ref() != render_expr {
                    self.render_expr.set(render_expr);
                }
                self.structure_ready.notify_waiters();
            }
            UiEvent::Data { batch, generation } => {
                if generation != self.rows.generation() {
                    return;
                }
                // First batch of a new generation = the full authoritative
                // snapshot of the (possibly different) query. Apply it, then
                // retain only its keys so rows the new query no longer
                // returns are dropped (apply-then-retain: surviving rows keep
                // their Mutable cell identity and the set is never empty
                // in between).
                let is_first_batch_of_generation = self
                    .data_generation
                    .swap(generation, std::sync::atomic::Ordering::AcqRel)
                    != generation;
                let mut snapshot_keys: Vec<EntityUri> = Vec::new();
                for change in batch.inner.items {
                    // UiEvent::Data carries Change<DataRow> (MapChange) for FFI compat,
                    // but the data was enriched by forward_data_stream → enrich_batch
                    // before being packed into the UiEvent.
                    //
                    // Re-wrap via from_raw with no-op computed fields since enrichment
                    // already happened. This is the ONE remaining seam — eliminating it
                    // requires changing UiEvent::Data to carry EnrichedChange directly.
                    let enriched = change.map(|data| {
                        EnrichedRow::from_raw(data, |_| std::collections::HashMap::new())
                    });
                    if is_first_batch_of_generation {
                        if let holon_api::Change::Created { data, .. } = &enriched {
                            // Retain BOTH entity- and value-shaped rows: the
                            // snapshot key is the row's `RowIdentity` store key
                            // (content hash for id-less value rows), so an
                            // aggregate / rule-trigger row is not dropped by the
                            // post-snapshot `retain_keys`.
                            snapshot_keys.push(holon_api::RowIdentity::of_row(data).to_store_key());
                        }
                    }
                    self.rows.apply_change(enriched, generation);
                }
                if is_first_batch_of_generation {
                    self.rows.retain_keys(&snapshot_keys);
                }
            }
        }
    }

    /// Apply a single enriched row-level CDC change. Delegates to the inner row
    /// set.
    pub fn apply_change(&self, change: holon_api::Change<EnrichedRow>, generation: u64) {
        self.rows.apply_change(change, generation);
    }

    /// Synchronous snapshot of current state (Arc-wrapped rows, cheap).
    pub fn snapshot(&self) -> (RenderExpr, Vec<Arc<DataRow>>) {
        let expr = self.render_expr.get_cloned();
        (expr, self.rows.snapshot_rows())
    }

    /// Per-row `SignalVec`. Delegates to the inner row set.
    pub fn row_signal_vec(&self) -> impl SignalVec<Item = Arc<DataRow>> {
        self.rows.row_signal_vec()
    }

    /// Signal that fires the full row set whenever any row changes.
    pub fn data_signal(&self) -> impl Signal<Item = Vec<(EntityUri, Arc<DataRow>)>> {
        self.rows.data_signal()
    }

    /// Per-row keyed `SignalVec`. Delegates to the inner row set.
    pub fn keyed_signal_vec(&self) -> impl SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> {
        self.rows.keyed_signal_vec()
    }

    /// Signal that emits a new `ReactiveViewModel` whenever render_expr or data
    /// changes.
    ///
    /// `interpret_fn` transforms `(&RenderExpr, &[Arc<DataRow>]) →
    /// ReactiveViewModel`.
    ///
    /// **Note**: This re-interprets the ENTIRE tree on every change (structural
    /// OR data). For per-row collection updates, use `structural_signal()`
    /// + `ReactiveCollection`.
    pub fn reactive_signal<F: ?Sized>(
        &self,
        interpret_fn: Arc<F>,
    ) -> impl Signal<Item = ReactiveViewModel>
    where
        F: Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    {
        self.reactive_signal_with_ui_gen(interpret_fn, futures_signals::signal::always(0u64))
    }

    /// Like `reactive_signal` but also re-interprets when `ui_gen_signal`
    /// fires.
    ///
    /// Used by `ReactiveEngine` to include `UiState.ui_generation` in the
    /// signal graph so that focus/view-mode changes trigger re-interpretation.
    pub fn reactive_signal_with_ui_gen<F: ?Sized>(
        &self,
        interpret_fn: Arc<F>,
        ui_gen_signal: impl Signal<Item = u64> + Send + 'static,
    ) -> impl Signal<Item = ReactiveViewModel>
    where
        F: Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    {
        let expr_signal = self.render_expr.signal_cloned();
        let data_signal = self.rows.data_signal();

        map_ref! {
            let expr = expr_signal,
            let entries = data_signal,
            let _ui_gen = ui_gen_signal
            => {
                let rows: Vec<Arc<DataRow>> = entries.iter().map(|(_, v)| Arc::clone(v)).collect();
                interpret_fn(expr, &rows)
            }
        }
    }

    /// Signal that fires ONLY on structural changes (render_expr).
    ///
    /// Data-only changes do NOT trigger re-interpretation. Instead, the caller
    /// sets up a `ReactiveCollection` subscribed to `row_signal_vec()` for
    /// per-row updates to the tree's `MutableVec` items.
    ///
    /// The current data snapshot is read at interpretation time, so the initial
    /// tree is correct. Subsequent data changes are handled by the collection.
    pub fn structural_signal<F: ?Sized>(
        &self,
        interpret_fn: Arc<F>,
    ) -> impl Signal<Item = ReactiveViewModel>
    where
        F: Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    {
        let rows = &self.rows;
        let data = rows.data.clone();
        self.render_expr.signal_cloned().map(move |expr| {
            let rows: Vec<Arc<DataRow>> = data
                .lock_ref()
                .iter()
                .map(|(_, cell)| cell.get_cloned())
                .collect();
            interpret_fn(&expr, &rows)
        })
    }

    /// Like `structural_signal` but also re-interprets when `ui_gen_signal`
    /// fires.
    ///
    /// Fires on render_expr change OR ui_state change (focus, view mode).
    /// Data-only changes do NOT trigger — those are handled by ReactiveView
    /// drivers.
    pub fn structural_signal_with_ui_gen<F: ?Sized>(
        &self,
        interpret_fn: Arc<F>,
        ui_gen_signal: impl Signal<Item = u64> + Send + 'static,
    ) -> impl Signal<Item = ReactiveViewModel>
    where
        F: Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    {
        let expr_signal = self.render_expr.signal_cloned();
        let data = self.rows.data.clone();

        map_ref! {
            let expr = expr_signal,
            let _ui_gen = ui_gen_signal
            => {
                let rows: Vec<Arc<DataRow>> = data
                    .lock_ref()
                    .iter()
                    .map(|(_, cell)| cell.get_cloned())
                    .collect();
                interpret_fn(expr, &rows)
            }
        }
    }
}

impl ReactiveRowProvider for ReactiveRenderedRows {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        self.rows.snapshot_rows()
    }
    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        Box::pin(self.rows.row_signal_vec())
    }
    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> + Send>> {
        Box::pin(self.rows.keyed_signal_vec())
    }
    fn row_mutable(&self, id: &EntityUri) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
        self.rows.row_mutable(id)
    }
    fn cache_identity(&self) -> u64 {
        // Inner row-set pointer — two `ReactiveRenderedRows` wrapping
        // the same rows would share identity, which is what the cache
        // wants.
        ptr_identity(&self.rows)
    }
}

// ── ReactiveRegistry ─────────────────────────────────────────────────────

/// Internal registry of ReactiveRenderedRows, keyed by EntityUri.
///
/// Thread-safe: the HashMap is behind a Mutex, but individual
/// ReactiveRenderedRows fields use futures-signals' lock-free primitives.
struct ReactiveRegistry {
    entries: Mutex<HashMap<EntityUri, Arc<ReactiveRenderedRows>>>,
}

impl ReactiveRegistry {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_create(&self, id: &EntityUri) -> Arc<ReactiveRenderedRows> {
        self.entries
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert_with(|| Arc::new(ReactiveRenderedRows::new()))
            .clone()
    }

    fn remove(&self, id: &EntityUri) {
        self.entries.lock().unwrap().remove(id);
    }
}

// ── WatcherState ─────────────────────────────────────────────────────────

struct WatcherState {
    task: tokio::task::JoinHandle<()>,
    command_tx: tokio::sync::mpsc::Sender<holon_api::WatcherCommand>,
    /// Number of live [`WatchGuard`]s pinning this watcher. Read-only paths
    /// ([`ReactiveEngine::ensure_watching`]) never touch this — a watcher
    /// started by a read alone sits at 0 (warm cache) and is only reclaimed
    /// once an acquired count drops back to 0.
    refcount: usize,
}

// ── WatchGuard ───────────────────────────────────────────────────────────

/// RAII pin on a block/query watcher.
///
/// Acquired via [`ReactiveEngine::acquire_watch`], or carried inside the
/// [`LiveBlock`] returned by [`ReactiveEngine::watch_live`] /
/// [`ReactiveEngine::watch_query_live`]. Dropping the last guard for a key
/// aborts the watcher task and releases its reactive state. Long-lived
/// subscribers (shells, views) hold the guard for as long as they consume
/// the watch; one-shot readers must NOT acquire one — they use the
/// non-counting [`ReactiveEngine::ensure_watching`] read path instead.
#[must_use = "dropping the guard releases the watcher; hold it for the lifetime of the subscription"]
pub struct WatchGuard {
    key: EntityUri,
    services: Arc<dyn BuilderServices>,
}

impl WatchGuard {
    fn new(key: EntityUri, services: Arc<dyn BuilderServices>) -> Self {
        Self { key, services }
    }

    /// The watcher key this guard pins (a block URI, or a synthetic
    /// `query:<hash>` key for query watchers).
    pub fn key(&self) -> &EntityUri {
        &self.key
    }
}

impl std::fmt::Debug for WatchGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WatchGuard").field(&self.key).finish()
    }
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.services.unwatch(&self.key);
    }
}

// ── UiState ──────────────────────────────────────────────────────────────

/// Frontend-local UI state for predicate evaluation.
///
/// Tracks which block is focused and per-block view modes. Changes to these
/// values trigger re-interpretation of affected blocks — no backend round-trip.
///
/// All fields use futures-signals `Mutable` types so that signal graph
/// consumers can react to changes automatically.
/// Frontend-owned viewport information, pushed in by the platform shell
/// whenever the root drawing area changes: window resize (desktop), keyboard
/// show/hide (mobile), orientation change, split-screen, safe-area changes.
///
/// `width_px` / `height_px` are **logical pixels** — already DPI-normalized
/// by the UI framework. `scale_factor` is the device pixel ratio, so
/// `width_px * scale_factor` gives physical pixels for density-aware
/// decisions. No physical-size (cm/inch) measurement: logical px is
/// sufficient for "phone vs tablet vs desktop" breakpoints and is
/// trivially available on every platform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportInfo {
    pub width_px: f32,
    pub height_px: f32,
    pub scale_factor: f32,
}

pub struct UiState {
    /// Currently focused block (receives `is_focused = true` in predicate
    /// context).
    focused_block: Mutable<Option<EntityUri>>,
    /// Monotonically increasing counter, bumped when the viewport changes.
    /// Included in `ReactiveRenderedRows::reactive_signal` so that viewport
    /// changes trigger re-interpretation of affected blocks (breakpoint
    /// updates). View mode and expand state are now handled by node-owned
    /// `Mutable`s (no engine-level caches).
    viewport_generation: Mutable<u64>,
    /// Root viewport allocation. The platform shell pushes updates here on
    /// resize / keyboard / rotation events; the root `ReactiveView`'s
    /// `space` Mutable mirrors this, which starts the container-query
    /// cascade down through partitioning layout containers.
    viewport: Mutable<Option<ViewportInfo>>,
    /// One-shot initial caret offset for the *next* editor to mount for a
    /// given block. Set alongside `focused_block` by focus moves that need a
    /// non-default caret position (split → 0, join → join boundary,
    /// cross-block nav → placement offset). Consumed and cleared by the
    /// mounting editor; if absent, the editor defaults to end-of-text. This
    /// is how the initial caret reaches a backend-driven mount in-process,
    /// replacing the old `editor_cursor` round-trip.
    pending_caret_seed: Mutable<Option<(EntityUri, usize)>>,
    /// SPIKE (Phase 1b — display-placement de-risk): which *occurrence* of
    /// `focused_block` holds focus. `None` = the block's canonical occurrence
    /// (every existing `set_focus` path leaves this `None`, so behaviour is
    /// unchanged — this is deliberately ADDITIVE). `Some(n)` = a display-placed
    /// occurrence. This proves the focus authority can carry `(id, occurrence)`
    /// WITHOUT widening `focused_block`'s type (which would ripple through ~10
    /// readers + all four frontends — ADR 0010's reserved graduation).
    focused_occurrence: Mutable<Option<u32>>,
    /// Monotonic counter bumped ONLY when navigation opens a *different page*
    /// in the `main` region (`navigation.focus` region=main, or
    /// `navigation.go_home`). Read by the GPUI frontend to reset the main
    /// panel's scroll to the top on page change — distinct from
    /// `focused_block`, which also moves on same-page block clicks (which
    /// must NOT reset scroll). Not a render signal; polled during render.
    main_nav_generation: Mutable<u64>,
    /// View-local expansion state for `expand_toggle` widgets, keyed by the
    /// widget's `target_id` (bare block id). Purely a VIEW concern (RATIFIED
    /// 2026-07-16, Option B): never persisted, never written to the document,
    /// no `collapsed` column involved. The `expand_toggle` shadow builder seeds
    /// its `expanded` gate from this on (re)build when an entry exists,
    /// otherwise keeps the collapsed-until-clicked default. This is the ONLY
    /// mechanism that survives a fresh `snapshot()` for profile-driven embedded
    /// pages (whose `expand_toggle` is synthesized during recursive resolve and
    /// carries no `collapsed` field). NB this deliberately reintroduces a small
    /// engine-level view cache for expand state, which an earlier note above
    /// ("no engine-level caches") had removed — see the ruling.
    expanded_view: Mutex<HashMap<String, bool>>,
    /// Fail-loud companion to [`Self::expanded_view`]: every `target_id` an
    /// `expand_toggle` builder has read a seed for since the last write. Lets a
    /// driver detect a view-state write for a target that renders no
    /// `expand_toggle` (the write would otherwise be silently absorbed).
    expanded_view_observed: Mutex<std::collections::HashSet<String>>,
}

impl UiState {
    fn new() -> Self {
        Self {
            focused_block: Mutable::new(None),
            viewport_generation: Mutable::new(0),
            viewport: Mutable::new(None),
            pending_caret_seed: Mutable::new(None),
            focused_occurrence: Mutable::new(None),
            main_nav_generation: Mutable::new(0),
            expanded_view: Mutex::new(HashMap::new()),
            expanded_view_observed: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Seed value for an `expand_toggle` whose `target_id` is `target_id`, or
    /// `None` when the user has never driven this toggle (builder then keeps
    /// its collapsed-until-clicked default). Records the read so a driver
    /// can tell the toggle actually rendered (fail-loud companion to
    /// [`Self::set_block_expanded_view`]).
    pub(crate) fn block_expanded_view(&self, target_id: &str) -> Option<bool> {
        // Normalize the key: the driver strips the `block:` scheme while the
        // `expand_toggle` builder reads the (schemed) row `id`. Keying on the
        // bare id makes both sides agree.
        let key = target_id.strip_prefix("block:").unwrap_or(target_id);
        self.expanded_view_observed
            .lock()
            .unwrap()
            .insert(key.to_string());
        self.expanded_view.lock().unwrap().get(key).copied()
    }

    /// Record view-local expansion intent for `target_id` and bump
    /// `viewport_generation` so mounted frontends re-render (the same
    /// invalidation breakpoint/viewport changes use). Clears the observed flag
    /// so a subsequent re-render can confirm the toggle actually re-rendered.
    pub(crate) fn set_block_expanded_view(&self, target_id: &str, expanded: bool) {
        let key = target_id.strip_prefix("block:").unwrap_or(target_id);
        self.expanded_view
            .lock()
            .unwrap()
            .insert(key.to_string(), expanded);
        self.expanded_view_observed.lock().unwrap().remove(key);
        self.viewport_generation
            .set(self.viewport_generation.get() + 1);
    }

    /// Whether an `expand_toggle` builder has read a seed for `target_id` since
    /// the last [`Self::set_block_expanded_view`] — i.e. the target's toggle
    /// actually (re)rendered.
    pub(crate) fn expand_toggle_observed(&self, target_id: &str) -> bool {
        let key = target_id.strip_prefix("block:").unwrap_or(target_id);
        self.expanded_view_observed.lock().unwrap().contains(key)
    }

    /// Current main-region navigation generation. Bumped on each page change in
    /// the `main` region; see [`Self::bump_main_nav`].
    pub fn main_nav_generation(&self) -> u64 {
        self.main_nav_generation.get()
    }

    /// Record that a *page navigation* landed in the `main` region. Bumps
    /// [`Self::main_nav_generation`] so the frontend resets main-panel scroll.
    fn bump_main_nav(&self) {
        self.main_nav_generation
            .set(self.main_nav_generation.get() + 1);
    }

    /// SPIKE: set the focused occurrence alongside the focused block. `None`
    /// restores canonical focus. Additive — no effect on `focused_block`.
    pub(crate) fn set_focus_occurrence(&self, occ: Option<u32>) {
        if self.focused_occurrence.get_cloned() != occ {
            self.focused_occurrence.set(occ);
        }
    }

    /// SPIKE: read the currently focused occurrence (`None` = canonical).
    pub(crate) fn focused_occurrence(&self) -> Option<u32> {
        self.focused_occurrence.get_cloned()
    }

    /// Get a signal that fires when the viewport changes. Include in reactive
    /// signal combinators to trigger re-interpretation on window resize and
    /// breakpoint changes.
    pub fn generation_signal(&self) -> impl Signal<Item = u64> {
        self.viewport_generation.signal()
    }

    /// Set the currently focused block. Pass `None` to clear focus.
    ///
    /// Does NOT bump `ui_generation`. Focus is pure UI state that GPUI
    /// handles via `window.focus()` — the old editor's `on_blur` stops its
    /// blink cursor, the new editor's `on_focus` starts it. Bumping
    /// `ui_generation` would cause live-query shells to replace their entire
    /// tree (re-creating editors for all 269 rows), producing multiple
    /// cursors. Mutate the focused-block signal directly. Crate-private:
    /// external callers (test code, frontend impls) MUST go through the
    /// `navigation.focus` / `navigation.editor_focus` / `navigation.go_home`
    /// intent so that `maybe_mirror_navigation_focus` keeps the SQL
    /// nav-history table in sync. Test-side direct calls were removed
    /// in `frontends/tui/TODO.md` items A2–A5; this visibility change
    /// (item B1) closes the door so it can't be reopened by accident.
    pub(crate) fn set_focus(&self, block_id: Option<EntityUri>) {
        // Drop a stale caret seed unless it targets the block we're focusing
        // (so a plain click defaults that block's caret to end-of-text, while
        // a `set_focus_with_caret` that pre-armed the seed for this same block
        // keeps it). The seed is read non-destructively, so this set/clear is
        // the only thing that ages it out.
        if let Some((ref seed_block, _)) = self.pending_caret_seed.get_cloned() {
            if Some(seed_block) != block_id.as_ref() {
                self.pending_caret_seed.set(None);
            }
        }
        if self.focused_block.get_cloned() == block_id {
            return;
        }
        self.focused_block.set(block_id);
    }

    /// Move focus to `block` and arm a one-shot initial caret offset for the
    /// editor that mounts for it. Used by focus moves whose caret must NOT
    /// default to end-of-text — split (offset 0), join (join boundary),
    /// cross-block nav (placement offset). The mounting editor reads the seed
    /// via [`peek_caret_seed`](Self::peek_caret_seed) (non-destructively, so
    /// both the synchronous first-mount grab and the focus subscription can
    /// apply it idempotently).
    pub(crate) fn set_focus_with_caret(&self, block: EntityUri, offset: usize) {
        self.pending_caret_seed.set(Some((block.clone(), offset)));
        self.set_focus(Some(block)); // keeps the seed (same block)
    }

    /// Cloned `Mutable` handles for the focus signal + pending caret seed.
    /// Used by the dispatch result-hook, which runs in a spawned task and
    /// can't borrow `&self`. `Mutable` clones share state.
    fn focus_handles(
        &self,
    ) -> (
        Mutable<Option<EntityUri>>,
        Mutable<Option<(EntityUri, usize)>>,
    ) {
        (self.focused_block.clone(), self.pending_caret_seed.clone())
    }

    /// Cloned handles for the focus signal + main-region nav generation.
    /// Used by [`ReactiveEngine::follow_dangling_link`], whose create→navigate
    /// chain runs in a spawned task (so it can't borrow `&self`) and mirrors an
    /// ordinary `navigation.focus` into these two `Mutable`s once the newly
    /// created page's id is known from the create op's response.
    fn nav_focus_handles(&self) -> (Mutable<Option<EntityUri>>, Mutable<u64>) {
        (self.focused_block.clone(), self.main_nav_generation.clone())
    }

    /// Read the pending caret seed for `block` without consuming it. Returns
    /// the armed offset, or `None` (caller defaults the caret to end-of-text).
    /// Non-destructive so the first-mount grab and the focus subscription can
    /// both apply it without racing; the seed is aged out by [`set_focus`]
    /// when focus moves to a different block.
    pub fn peek_caret_seed(&self, block: &EntityUri) -> Option<usize> {
        match self.pending_caret_seed.get_cloned() {
            Some((ref b, offset)) if b == block => Some(offset),
            _ => None,
        }
    }

    /// Consume (clear) the caret seed if it targets `block`. Called by the
    /// mounting editor the moment it applies the seed, making the seed strictly
    /// single-use so a later click cannot re-apply the stale op-follow-up
    /// offset (the caret-0/prepend corruption after split+join, BugFunnel
    /// 2026-07-11 row 80). A no-op when the armed seed targets a different
    /// block — that block's mount will consume its own seed. Unlike
    /// [`set_focus`]'s aging (which only clears on a focus MOVE to a
    /// different block), this clears even while focus stays put, closing
    /// the window where a "failed click elsewhere" leaves the seed armed
    /// for a re-click.
    pub fn consume_caret_seed(&self, block: &EntityUri) {
        if let Some((ref seed_block, _)) = self.pending_caret_seed.get_cloned() {
            if seed_block == block {
                self.pending_caret_seed.set(None);
            }
        }
    }

    /// Get the currently focused block ID.
    pub fn focused_block(&self) -> Option<EntityUri> {
        self.focused_block.get_cloned()
    }

    /// Cloned handle to the focused-block `Mutable`. Used by reactive
    /// row providers (`focus_chain`) that need both a synchronous
    /// snapshot and a long-lived signal source. `Mutable` clones share
    /// state.
    pub fn focused_block_mutable(&self) -> Mutable<Option<EntityUri>> {
        self.focused_block.clone()
    }

    /// Update the root viewport. Called by the platform shell on window
    /// resize, keyboard show/hide, orientation change, etc.
    ///
    /// Bumps `viewport_generation` because breakpoint changes alter the
    /// selected variant and therefore the render expression (structural
    /// change). `Mutable::set_neq` dedups equal values — no-op updates
    /// never fire the signal graph.
    pub fn set_viewport(&self, info: ViewportInfo) {
        if self.viewport.get_cloned() == Some(info) {
            return;
        }
        self.viewport.set(Some(info));
        self.viewport_generation
            .set(self.viewport_generation.get() + 1);
    }

    /// Get a snapshot of the current viewport.
    pub fn viewport(&self) -> Option<ViewportInfo> {
        self.viewport.get_cloned()
    }

    /// Get a signal for the current viewport — used by the root
    /// `ReactiveView` to mirror viewport changes into its `space` Mutable.
    pub fn viewport_signal(&self) -> impl Signal<Item = Option<ViewportInfo>> {
        self.viewport.signal()
    }

    /// Build a predicate evaluation context for a given block.
    ///
    /// Returns a `HashMap<String, Value>` with:
    /// - `is_focused`: true if this block is the focused block
    /// - viewport variables (viewport_width_px, etc.)
    ///
    /// Note: `view_mode` and `is_expanded` are added by
    /// `ReactiveEngine::ui_state()` from the engine's keyed caches, not
    /// here.
    pub fn context_for(&self, block_id: &EntityUri) -> HashMap<String, holon_api::Value> {
        let mut ctx = HashMap::new();

        let is_focused = self
            .focused_block
            .get_cloned()
            .as_ref()
            .map_or(false, |f| f == block_id);
        ctx.insert(
            "is_focused".to_string(),
            holon_api::Value::Boolean(is_focused),
        );

        // ALLOW(fallback): the seeded global viewport is intentionally
        // emitted as a default-value baseline that per-subtree writes from
        // `pick_active_variant` then shadow — see CLAUDE.md "Falls back
        // visibly — clearly signals degraded mode" — so blocks not reached
        // by any partitioning container's space cascade still have a
        // viewport context to evaluate `viewport_*` predicates against.
        // Global viewport baseline: emitted so blocks not reached by any
        // partitioning container's space cascade still have something to
        // evaluate their `viewport_*` predicates against. Per-subtree
        // `available_*` values written by `pick_active_variant` shadow
        // these (they're merged *after* `context_for`).
        if let Some(vp) = self.viewport.get_cloned() {
            ctx.insert(
                "viewport_width_px".to_string(),
                holon_api::Value::Float(vp.width_px as f64),
            );
            ctx.insert(
                "viewport_height_px".to_string(),
                holon_api::Value::Float(vp.height_px as f64),
            );
            ctx.insert(
                "viewport_width_physical_px".to_string(),
                holon_api::Value::Float((vp.width_px * vp.scale_factor) as f64),
            );
            ctx.insert(
                "viewport_height_physical_px".to_string(),
                holon_api::Value::Float((vp.height_px * vp.scale_factor) as f64),
            );
            ctx.insert(
                "scale_factor".to_string(),
                holon_api::Value::Float(vp.scale_factor as f64),
            );
        }

        ctx
    }
}

// ── ReactiveEngine ───────────────────────────────────────────────────────

/// The reactive middle layer.
///
/// Manages per-block `ReactiveRenderedRows` instances. Each block's UiEvent
/// stream feeds its ReactiveRenderedRows; the signal graph produces ViewModels
/// on demand. Frontends consume via `watch()` (stream) or `snapshot()`
/// (polling). TODO: This looks like a god-class heavily violating SRP
pub struct ReactiveEngine {
    registry: ReactiveRegistry,
    session: Arc<FrontendSession>,
    pub runtime_handle: tokio::runtime::Handle,
    interpret_fn: Arc<dyn Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync>,
    /// The shared shadow interpreter, built once by
    /// `HolonFrontendModule::configure()` and injected here via DI. Used by
    /// `BuilderServices::interpret`.
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    watchers: Mutex<HashMap<EntityUri, WatcherState>>,
    ui_state: UiState,
    /// Reactive keybinding registry: operation_name → key chord.
    /// Keybindings are joined into OperationDescriptors during ViewModel
    /// construction.
    key_bindings: MutableBTreeMap<String, holon_api::KeyChord>,
    /// Shared Weak-ref cache of `ReactiveRowProvider`s produced by
    /// value functions like `focus_chain()` / `ops_of(uri)`. Reused
    /// across render passes so identical `(name, args)` calls share an
    /// Arc instead of each building a fresh provider.
    provider_cache: Arc<crate::provider_cache::ProviderCache>,
    /// Optional entity-cell registry (e.g. holon's Loro-backed
    /// `BlockCellRegistry`). When `Some`, `BuilderServices::editable_text()`
    /// resolves `live_field::<String>(EntityUri::block(id), "content")`
    /// through it. Set by frontend DI factories that want CRDT-backed editors.
    pub block_cell_registry: Mutex<Option<Arc<dyn crate::cell::EntityCellRegistry>>>,
    /// Shared slot used to recover an owned `Arc<dyn BuilderServices>` from
    /// inside `&self` methods. Populated by the owning frontend right after
    /// the engine is wrapped in an Arc; `clone_arc()` reads it.
    pub services_slot: Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>>,

    /// The session-level advice weave sidecar (ADR 0022): anchor → synthesized
    /// advice rows, read synchronously and purely by
    /// [`BuilderServices::advice_children`] during interpretation. Populated by
    /// [`Self::refresh_advice_sidecar`] (deterministic settle) and by the
    /// lazily spawned reactive weaver
    /// ([`crate::advice_weaver::spawn_session_weaver`]), both writing this
    /// same map. Empty when no active rule matches — the snapshot then
    /// stays byte-identical to a pre-advice render.
    advice_sidecar: crate::advice_weaver::AdviceSidecar,
    /// Guards the one-time lazy spawn of the reactive advice weaver (spawned on
    /// first `advice_children` call, once a query engine is wired).
    advice_weaver_started: std::sync::atomic::AtomicBool,
}

impl ReactiveEngine {
    pub fn new(
        session: Arc<FrontendSession>,
        runtime_handle: tokio::runtime::Handle,
        interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
        interpret_fn: impl Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
        services_slot: Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>>,
    ) -> Self {
        use holon_api::input_types::Key;

        let key_bindings = MutableBTreeMap::new();
        {
            let mut bindings = key_bindings.lock_mut();
            bindings.insert_cloned(
                "cycle_task_state".into(),
                holon_api::KeyChord::new(&[Key::Cmd, Key::Enter]),
            );
            bindings.insert_cloned(
                "split_block".into(),
                holon_api::KeyChord::new(&[Key::Enter]),
            );
            bindings.insert_cloned(
                "join_block".into(),
                holon_api::KeyChord::new(&[Key::Backspace]),
            );
            bindings.insert_cloned("indent".into(), holon_api::KeyChord::new(&[Key::Tab]));
            bindings.insert_cloned(
                "outdent".into(),
                holon_api::KeyChord::new(&[Key::Shift, Key::Tab]),
            );
            bindings.insert_cloned(
                "move_up".into(),
                holon_api::KeyChord::new(&[Key::Alt, Key::Up]),
            );
            bindings.insert_cloned(
                "move_down".into(),
                holon_api::KeyChord::new(&[Key::Alt, Key::Down]),
            );
        }

        Self {
            registry: ReactiveRegistry::new(),
            session,
            runtime_handle,
            interpret_fn: Arc::new(interpret_fn),
            interpreter,
            watchers: Mutex::new(HashMap::new()),
            ui_state: UiState::new(),
            key_bindings,
            provider_cache: Arc::new(crate::provider_cache::ProviderCache::new()),
            block_cell_registry: Mutex::new(None),
            services_slot,
            advice_sidecar: Arc::new(Mutex::new(HashMap::new())),
            advice_weaver_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Recompute the advice weave sidecar from the current SQL state (one-shot
    /// canonical read over every anchor the active rule produces). Called by
    /// the composed settle to converge the weave deterministically before a
    /// snapshot; also the recompute the reactive weaver runs on each
    /// trigger. A no-Turso (Loro-only) session has no query engine → the
    /// sidecar stays empty.
    pub async fn refresh_advice_sidecar(&self) {
        match self.session.query_engine() {
            Some(query_engine) => {
                crate::advice_weaver::recompute_sidecar(query_engine.as_ref(), &self.advice_sidecar)
                    .await
            }
            None => self.advice_sidecar.lock().unwrap().clear(),
        }
    }

    /// Lazily start the reactive advice weaver exactly once, as soon as a query
    /// engine is wired. Idempotent and cheap on the hot path (an
    /// already-started atomic load). A session that never wires a query
    /// engine never spawns it.
    fn ensure_advice_weaver(&self) {
        use std::sync::atomic::Ordering;
        if self.advice_weaver_started.load(Ordering::Relaxed) {
            return;
        }
        let Some(query_engine) = self.session.query_engine() else {
            return;
        };
        if self.advice_weaver_started.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::advice_weaver::spawn_session_weaver(
            &self.runtime_handle,
            query_engine,
            self.advice_sidecar.clone(),
        );
    }

    /// Access the shared `ReactiveRowProvider` cache. Value functions
    /// that construct providers (`focus_chain`, `ops_of`, ...) route
    /// through this cache via `get_or_create` so callers that produce
    /// the same `(name, args)` share an Arc.
    pub fn provider_cache(&self) -> &crate::provider_cache::ProviderCache {
        &self.provider_cache
    }

    /// Access the UI state for focus/view-mode management.
    pub fn ui_state(&self) -> &UiState {
        &self.ui_state
    }

    /// Access the reactive keybinding registry.
    pub fn key_bindings(&self) -> &MutableBTreeMap<String, holon_api::KeyChord> {
        &self.key_bindings
    }

    /// Start watching a block and return a `Signal<ReactiveViewModel>`.
    ///
    /// The signal re-evaluates when the block's render expression or data
    /// changes. Poll this directly from a GPUI `cx.spawn` — no intermediate
    /// channel needed. CDC writes from tokio wake the signal cross-thread.
    pub fn watch_signal(
        &self,
        block_id: &EntityUri,
    ) -> Pin<Box<dyn Signal<Item = ReactiveViewModel> + Send>> {
        let results = self.ensure_watching(block_id);
        results
            .reactive_signal_with_ui_gen(
                self.interpret_fn.clone(),
                self.ui_state.generation_signal(),
            )
            .boxed()
    }

    /// Watch a block's data and structure, but NOT ui_generation.
    ///
    /// Unlike `watch_signal`, this does NOT react to `ui_generation` changes
    /// (focus, view_mode). Use for the root layout and other containers whose
    /// interpretation doesn't depend on UI state — avoids the full re-render
    /// cascade that `watch_signal` triggers on every focus change.
    pub fn watch_data_signal(
        &self,
        block_id: &EntityUri,
    ) -> Pin<Box<dyn Signal<Item = ReactiveViewModel> + Send>> {
        let results = self.ensure_watching(block_id);
        results.reactive_signal(self.interpret_fn.clone()).boxed()
    }

    /// Start watching a block and return a `Stream<Item = ReactiveViewModel>`.
    ///
    /// Convenience wrapper over `watch_signal()` for consumers that need a
    /// Stream (non-GPUI frontends, tests). Prefer `watch_signal()` +
    /// `for_each` for GPUI.
    pub fn watch(
        &self,
        block_id: &EntityUri,
    ) -> Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>> {
        Box::pin(self.watch_signal(block_id).to_stream())
    }

    /// Watch a block for snapshot-pipeline frontends (the wasm worker → web
    /// page path): unlike [`Self::watch`], the stream ALSO re-fires when
    /// editor focus moves. `is_focused` is baked into every interpretation
    /// (`UiState::context_for`), and a snapshot consumer has no live widget
    /// tree with a focus driver to patch the `rendered_text` ⇄
    /// `editable_text` variant in place — re-interpreting is its only way
    /// to observe a focus change. GPUI must NOT use this: re-interpreting
    /// per focus change would recreate its editors (see the
    /// [`UiState::set_focus`] doc on multiple cursors).
    pub fn watch_snapshot_stream(
        &self,
        block_id: &EntityUri,
    ) -> Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>> {
        let results = self.ensure_watching(block_id);
        let vp_gen = self.ui_state.generation_signal();
        let focus = self.ui_state.focused_block_mutable().signal_cloned();
        let combined = map_ref! {
            let g = vp_gen,
            let f = focus
            => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                g.hash(&mut h);
                f.hash(&mut h);
                h.finish()
            }
        };
        Box::pin(
            results
                .reactive_signal_with_ui_gen(self.interpret_fn.clone(), combined)
                .to_stream(),
        )
    }

    /// Watch a block with per-row collection reactivity.
    ///
    /// Returns a `LiveBlock` whose `tree` contains `ReactiveChildren` with
    /// `MutableVec`s that are updated per-row in the background. The
    /// `structural_changes` stream emits only when the render expression
    /// changes — data-only changes update the tree in-place via the MutableVec.
    ///
    /// `services` must be `Arc<ReactiveEngine>` cast to `Arc<dyn
    /// BuilderServices>`. (Passed explicitly to avoid self-referential
    /// `Arc<Self>` inside the engine.)
    pub fn watch_live(
        &self,
        block_id: &EntityUri,
        services: Arc<dyn BuilderServices>,
    ) -> LiveBlock {
        let (results, watch_guard) = self.acquire_watch(block_id, services.clone());

        // Interpret the initial tree from current snapshot.
        // If render_expr is Loading (watcher hasn't delivered yet), return a
        // Interpret the initial tree from current snapshot.
        // If render_expr is still loading (watcher hasn't delivered yet), the
        // "loading" builder produces a simple empty widget — no collection
        // drivers to wire. The structural_changes stream delivers the real
        // tree when the first Structure event arrives.
        let (expr, rows) = results.snapshot();
        let expr_name = match &expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => name.as_str(),
            _ => "non-function",
        };
        tracing::debug!(
            "[watch_live] block={block_id}, expr={expr_name}, rows={}",
            rows.len()
        );
        let ctx = RenderContext {
            data_rows: rows.into(),
            data_source: Some(results.clone()),
            ..Default::default()
        };
        let tree = services.interpret(&expr, &ctx);
        // ReactiveView nodes inside the tree self-manage their drivers
        crate::reactive_view::start_reactive_views(&tree, &services, &self.runtime_handle);

        // Structural signal — fires when render_expr OR ui_state changes.
        // Builds a RenderContext with the live data_source so the macro
        // produces Streaming collections (not Static snapshots).
        let results_for_signal = results.clone();
        let services_for_signal = services.clone();
        let structural = results.structural_signal_with_ui_gen(
            Arc::new(move |expr: &RenderExpr, rows: &[Arc<DataRow>]| {
                let ctx = RenderContext {
                    data_rows: rows.to_vec().into(),
                    data_source: Some(results_for_signal.clone()),
                    ..Default::default()
                };
                services_for_signal.interpret(expr, &ctx)
            }),
            self.ui_state.generation_signal(),
        );
        let structural_stream = Box::pin(structural.to_stream());

        LiveBlock {
            tree,
            structural_changes: structural_stream,
            watch_guard: Some(watch_guard),
        }
    }

    /// Synchronous snapshot with resolved LiveBlock content.
    ///
    /// Interprets to `ReactiveViewModel`, then recursively resolves `LiveBlock`
    /// placeholders by calling `snapshot()` for each embedded block.
    /// Returns a fully-resolved static `ViewModel` for serialization consumers
    /// (MCP, PBT, TUI).
    ///
    /// Cycle detection via a thread-local visited set prevents stack overflow
    /// when block references form a cycle (e.g. A→B→A) or when the resolution
    /// chain exceeds the safe depth.
    pub fn snapshot(&self, block_id: &EntityUri) -> ViewModel {
        thread_local! {
            static VISITED: std::cell::RefCell<std::collections::HashSet<EntityUri>> =
                std::cell::RefCell::new(std::collections::HashSet::new());
            // Ordered call-stack alongside VISITED so we can log the full
            // resolution chain on cycle detection.
            static STACK: std::cell::RefCell<Vec<EntityUri>> =
                std::cell::RefCell::new(Vec::new());
        }

        // Try to enter: fail if already visiting this block_id (cycle detected).
        let entered = VISITED.with(|v| {
            let mut set = v.borrow_mut();
            if set.contains(block_id) {
                false
            } else {
                set.insert(block_id.clone());
                true
            }
        });

        if !entered {
            let stack: Vec<String> =
                STACK.with(|s| s.borrow().iter().map(|u| u.to_string()).collect());
            tracing::warn!(
                block_id = %block_id,
                stack = ?stack,
                "snapshot: self-reference in LiveBlock resolution; returning placeholder"
            );
            // Hitting an ancestor on the resolution stack is the legitimate
            // result of an unanchored query (e.g. GQL `MATCH (root:block) ...`
            // returns every block, including this one). Render a static
            // placeholder so the cycle terminates without breaking the
            // no-error-widget invariant. Outer `LiveBlock` wrapper is preserved
            // so structural consumers (focus, ancestor tracking, layout) still
            // see a live-block slot; the inner `Badge` is a leaf, so
            // `snapshot_resolved` does not recurse.
            return ViewModel::live_block(
                block_id.to_string(),
                ViewModel {
                    kind: crate::view_model::ViewKind::Badge {
                        label: format!("↺ self-reference: {block_id}"),
                    },
                    ..Default::default()
                },
            );
        }

        STACK.with(|s| s.borrow_mut().push(block_id.clone()));

        // Ensure we always remove from visited on exit (even on panic).
        struct Guard(EntityUri);
        impl Drop for Guard {
            fn drop(&mut self) {
                VISITED.with(|v| {
                    v.borrow_mut().remove(&self.0);
                });
                STACK.with(|s| {
                    let mut stack = s.borrow_mut();
                    if stack.last() == Some(&self.0) {
                        stack.pop();
                    }
                });
            }
        }
        let _guard = Guard(block_id.clone());

        let rvm = self.snapshot_reactive(block_id);
        rvm.snapshot_resolved(&|bid| self.snapshot(bid))
    }

    /// Synchronous reactive snapshot (placeholder LiveBlocks, not resolved).
    pub fn snapshot_reactive(&self, block_id: &EntityUri) -> crate::ReactiveViewModel {
        let results = self.ensure_watching(block_id);
        let (expr, rows) = results.snapshot();
        (self.interpret_fn)(&expr, &rows)
    }

    /// Get the ReactiveRenderedRows for a block, ensuring a watcher is running.
    ///
    /// **Non-counting read path**: starts the watcher if absent but does NOT
    /// pin it. The refcount tracks live [`WatchGuard`]s only, so one-shot
    /// readers (`snapshot_reactive`, `get_block_data`, `await_ready`, MCP /
    /// PBT / TUI snapshots) can call this arbitrarily often without leaking
    /// the watcher. A watcher started by a read alone stays warm at
    /// refcount 0 until a guard cycle reclaims it. Long-lived subscribers
    /// must pin the watcher via [`Self::acquire_watch`] (or hold the
    /// [`LiveBlock`] from [`Self::watch_live`], which carries the guard).
    pub fn ensure_watching(&self, block_id: &EntityUri) -> Arc<ReactiveRenderedRows> {
        let results = self.registry.get_or_create(block_id);
        let mut watchers = self.watchers.lock().unwrap();
        if !watchers.contains_key(block_id) {
            let state = self.spawn_block_watcher(block_id, results.clone(), 0);
            watchers.insert(block_id.clone(), state);
        }
        results
    }

    /// Counting acquisition: ensure the watcher is running AND pin it with an
    /// RAII [`WatchGuard`]. Dropping the last guard aborts the watcher task
    /// and releases the block's reactive state. `services` must resolve
    /// [`BuilderServices::unwatch`] to this engine — it is what the guard's
    /// `Drop` calls.
    pub fn acquire_watch(
        &self,
        block_id: &EntityUri,
        services: Arc<dyn BuilderServices>,
    ) -> (Arc<ReactiveRenderedRows>, WatchGuard) {
        let results = self.registry.get_or_create(block_id);
        let mut watchers = self.watchers.lock().unwrap();
        match watchers.get_mut(block_id) {
            Some(state) => state.refcount += 1,
            None => {
                let state = self.spawn_block_watcher(block_id, results.clone(), 1);
                watchers.insert(block_id.clone(), state);
            }
        }
        drop(watchers);
        (results, WatchGuard::new(block_id.clone(), services))
    }

    /// Test/diagnostic introspection: the number of live [`WatchGuard`]s
    /// pinning `block_id`'s watcher (`Some(0)` = read-warmed, unpinned;
    /// `None` = no watcher running).
    pub fn watcher_refcount(&self, block_id: &EntityUri) -> Option<usize> {
        self.watchers
            .lock()
            .unwrap()
            .get(block_id)
            .map(|s| s.refcount)
    }

    /// Test/diagnostic introspection: how many watcher tasks are running.
    pub fn active_watcher_count(&self) -> usize {
        self.watchers.lock().unwrap().len()
    }

    /// Spawn the CDC watcher task for `block_id`, feeding `reactive`.
    /// Callers hold the `watchers` lock and insert the returned state.
    fn spawn_block_watcher(
        &self,
        block_id: &EntityUri,
        reactive: Arc<ReactiveRenderedRows>,
        refcount: usize,
    ) -> WatcherState {
        let session = self.session.clone();
        let bid = block_id.clone();

        let (proxy_cmd_tx, mut proxy_cmd_rx) =
            tokio::sync::mpsc::channel::<holon_api::WatcherCommand>(16);

        let task = self.runtime_handle.spawn(async move {
            match session.watch_ui(&bid).await {
                Ok(watch) => {
                    // `_aborts` keeps the watcher's actor pipeline alive for as long
                    // as this task holds the receiver — dropping the guard cancels
                    // the merge_triggers actors, so it must outlive the recv loop.
                    let (mut event_rx, cmd_tx, _aborts) = watch.into_parts();

                    // Forward variant commands from engine → WatchHandle
                    tokio::spawn(async move {
                        while let Some(cmd) = proxy_cmd_rx.recv().await {
                            if cmd_tx.send(cmd).await.is_err() {
                                break;
                            }
                        }
                    });

                    while let Some(event) = event_rx.recv().await {
                        // Diagnostic: log every UiEvent for default-main-panel so we can
                        // see whether Data events arrive after focus changes.
                        if bid.as_str() == "block:default-main-panel" {
                            match &event {
                                UiEvent::Structure {
                                    render_expr,
                                    generation,
                                    ..
                                } => {
                                    let name = match render_expr {
                                        RenderExpr::FunctionCall { name, .. } => name.as_str(),
                                        _ => "non-fn",
                                    };
                                    tracing::trace!(
                                        "[mp_event] Structure gen={generation} expr={name}"
                                    );
                                }
                                UiEvent::Data { batch, generation } => {
                                    let cur_gen = reactive.rows.generation();
                                    let n = batch.inner.items.len();
                                    let dropped = *generation != cur_gen;
                                    tracing::trace!(
                                        "[mp_event] Data gen={generation} (current={cur_gen}) \
                                         items={n}{}",
                                        if dropped { " DROPPED-stale-gen" } else { "" }
                                    );
                                    // Per-change detail. Lets the next debugging session
                                    // confirm whether the matview CDC layer surfaces an
                                    // `Updated` for the modified block after split_block /
                                    // set_field — see HANDOFF_TUI_RENDER.md "third pass".
                                    for (i, change) in batch.inner.items.iter().enumerate() {
                                        let snippet =
                                            |row: &holon_api::widget_spec::DataRow| -> String {
                                                row.get("content")
                                                    .and_then(|v| v.as_string())
                                                    .map(|s| {
                                                        let s = s.replace('\n', "\\n");
                                                        if s.len() > 40 {
                                                            format!("{}…", &s[..40])
                                                        } else {
                                                            s
                                                        }
                                                    })
                                                    .unwrap_or_else(|| "<no content>".into())
                                            };
                                        match change {
                                            holon_api::Change::Created { data, .. } => {
                                                let id = data
                                                    .get("id")
                                                    .and_then(|v| v.as_string())
                                                    .unwrap_or("<no id>");
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: Created id={id} \
                                                     content={:?}",
                                                    snippet(data)
                                                );
                                            }
                                            holon_api::Change::Updated { id, data, .. } => {
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: Updated id={id} \
                                                     content={:?}",
                                                    snippet(data)
                                                );
                                            }
                                            holon_api::Change::Deleted { id, .. } => {
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: Deleted id={id}"
                                                );
                                            }
                                            holon_api::Change::FieldsChanged {
                                                entity_id,
                                                fields,
                                                ..
                                            } => {
                                                let names: Vec<&str> = fields
                                                    .iter()
                                                    .map(|(n, _, _)| n.as_str())
                                                    .collect();
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: FieldsChanged \
                                                     id={entity_id} fields={:?}",
                                                    names
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // Diagnostic for HANDOFF_DATA_CDC_SCOPE_LEAK.md: log each
                        // row applied to a tracked block's row set so we can see
                        // *which* data ids are flowing into which watcher.
                        // Gated by HOLON_TRACE_BLOCK_DATA=<substring>.
                        if let Ok(needle) = std::env::var("HOLON_TRACE_BLOCK_DATA") {
                            if !needle.is_empty() && bid.as_str().contains(&needle) {
                                if let UiEvent::Data {
                                    batch,
                                    generation: ev_gen,
                                } = &event
                                {
                                    let cur_gen = reactive.rows.generation();
                                    let ids: Vec<String> = batch
                                        .inner
                                        .items
                                        .iter()
                                        .filter_map(|c| match c {
                                            holon_api::Change::Created { data, .. } => data
                                                .get("id")
                                                .and_then(|v| v.as_string())
                                                .map(|s| s.to_string()),
                                            holon_api::Change::Updated { id, .. } => {
                                                Some(id.clone())
                                            }
                                            holon_api::Change::Deleted { id, .. } => {
                                                Some(format!("DEL:{id}"))
                                            }
                                            holon_api::Change::FieldsChanged {
                                                entity_id, ..
                                            } => Some(format!("FC:{entity_id}")),
                                        })
                                        .collect();
                                    tracing::warn!(
                                        block = %bid,
                                        relation = %batch.metadata.relation_name,
                                        ev_gen = ev_gen,
                                        cur_gen = cur_gen,
                                        n = batch.inner.items.len(),
                                        ids = ?ids,
                                        "[diag-cdc-leak] data batch"
                                    );
                                }
                            }
                        }
                        reactive.apply_event(event);
                        if bid.as_str() == "block:default-main-panel"
                            && tracing::enabled!(tracing::Level::TRACE)
                        {
                            let rows_n = reactive.rows.len();
                            tracing::trace!("[mp_event] post-apply rows.len={rows_n}");
                        }
                        static TRACE_BLOCK_DATA: std::sync::LazyLock<Option<String>> =
                            std::sync::LazyLock::new(|| {
                                std::env::var("HOLON_TRACE_BLOCK_DATA")
                                    .ok()
                                    .filter(|s| !s.is_empty())
                            });
                        if let Some(needle) = TRACE_BLOCK_DATA.as_ref() {
                            if bid.as_str().contains(needle.as_str()) {
                                let rows_n = reactive.rows.snapshot_rows().len();
                                let ids: Vec<String> = reactive
                                    .rows
                                    .snapshot_rows()
                                    .iter()
                                    .filter_map(|r| {
                                        r.get("id")
                                            .and_then(|v| v.as_string())
                                            .map(|s| s.to_string())
                                    })
                                    .collect();
                                tracing::warn!(
                                    block = %bid,
                                    rows_n = rows_n,
                                    ids = ?ids,
                                    "[diag-cdc-leak] post-apply row set"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("watch_ui({bid}) failed: {e}");
                }
            }
        });

        WatcherState {
            task,
            command_tx: proxy_cmd_tx,
            refcount,
        }
    }

    /// Watch a live query with per-row collection reactivity.
    ///
    /// Mirrors [`Self::watch_live`]: the tree is interpreted once with a live
    /// `data_source`, so collections inside it stream per-row diffs through
    /// their own `ReactiveView` pipelines. `structural_changes` fires only on
    /// render-expression or ui-generation changes. Returns the watcher key so
    /// the consumer can `unwatch` it on drop.
    pub fn watch_query_live(
        &self,
        query: String,
        lang: QueryLanguage,
        render_expr: RenderExpr,
        query_context: Option<crate::QueryContext>,
        services: Arc<dyn BuilderServices>,
    ) -> (EntityUri, LiveBlock) {
        let (key, results) = self.ensure_query_watching(query, lang, render_expr, query_context);
        // `ensure_query_watching` counted +1 for this call; the guard owns
        // that count and releases it on drop.
        let watch_guard = WatchGuard::new(key.clone(), services.clone());

        let (expr, rows) = results.snapshot();
        let ctx = RenderContext {
            data_rows: rows.into(),
            data_source: Some(results.clone()),
            ..Default::default()
        };
        let tree = services.interpret(&expr, &ctx);
        crate::reactive_view::start_reactive_views(&tree, &services, &self.runtime_handle);

        let results_for_signal = results.clone();
        let services_for_signal = services.clone();
        let structural = results.structural_signal_with_ui_gen(
            Arc::new(move |expr: &RenderExpr, rows: &[Arc<DataRow>]| {
                let ctx = RenderContext {
                    data_rows: rows.to_vec().into(),
                    data_source: Some(results_for_signal.clone()),
                    ..Default::default()
                };
                services_for_signal.interpret(expr, &ctx)
            }),
            self.ui_state.generation_signal(),
        );
        let structural_stream = Box::pin(structural.to_stream());

        (
            key,
            LiveBlock {
                tree,
                structural_changes: structural_stream,
                watch_guard: Some(watch_guard),
            },
        )
    }

    /// Ensure a query watcher is running and return its watcher key plus
    /// ReactiveRenderedRows. Every call counts +1 on the watcher refcount;
    /// [`Self::watch_query_live`] (the only caller) wraps that count in a
    /// [`WatchGuard`] whose drop releases it.
    fn ensure_query_watching(
        &self,
        query: String,
        lang: QueryLanguage,
        render_expr: RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> (EntityUri, Arc<ReactiveRenderedRows>) {
        // ALLOW(entity_uri_from_raw): synthetic 'query:<hash>' registry cache key (no
        // upstream EntityUri)
        let key = EntityUri::from_raw(&format!(
            "query:{}",
            hash_query(&query, lang, &query_context)
        ));
        let results = self.registry.get_or_create(&key);
        results.set_render_expr(render_expr);
        results.set_generation(1);

        let mut watchers = self.watchers.lock().unwrap();
        if let Some(state) = watchers.get_mut(&key) {
            state.refcount += 1;
            return (key, results);
        }
        {
            let session = self.session.clone();
            let reactive = results.clone();
            let task = self.runtime_handle.spawn(async move {
                match session
                    .watch_query(&query, lang, HashMap::new(), query_context)
                    .await
                {
                    Ok(stream) => {
                        let mut rx = stream.into_inner();
                        while let Some(batch) = rx.recv().await {
                            for enriched_change in batch.inner.items {
                                reactive.apply_change(enriched_change, 1);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("watch_query failed: {e}");
                    }
                }
            });

            let (dummy_tx, _) = tokio::sync::mpsc::channel(1);
            watchers.insert(
                key.clone(),
                WatcherState {
                    task,
                    command_tx: dummy_tx,
                    refcount: 1,
                },
            );
        }

        (key, results)
    }

    /// Send a variant switch command to a block's watcher.
    pub async fn set_variant(&self, block_id: &EntityUri, variant: String) -> anyhow::Result<()> {
        let watchers = self.watchers.lock().unwrap();
        let state = watchers
            .get(block_id)
            .ok_or_else(|| anyhow::anyhow!("No active watcher for {block_id}"))?;
        state
            .command_tx
            .send(holon_api::WatcherCommand::SetVariant(variant))
            .await
            .map_err(|_| anyhow::anyhow!("Watcher channel closed"))
    }

    /// Release one [`WatchGuard`]'s pin on a block's watcher. When the last
    /// guard drops, the watcher task is aborted and reactive state is
    /// released. Called by `WatchGuard::drop` — do not call manually; pair
    /// every count with a guard from [`Self::acquire_watch`] instead.
    pub fn unwatch(&self, block_id: &EntityUri) {
        let mut watchers = self.watchers.lock().unwrap();
        let should_remove = match watchers.get_mut(block_id) {
            Some(state) => {
                debug_assert!(
                    state.refcount > 0,
                    "unwatch({block_id}) without a matching acquire_watch — guard bookkeeping bug"
                );
                state.refcount = state.refcount.saturating_sub(1);
                state.refcount == 0
            }
            None => {
                // A guard outliving its (already reclaimed) watcher is a
                // bookkeeping bug — surface it, don't silently ignore.
                tracing::error!(
                    %block_id,
                    "unwatch for a block with no active watcher (guard/acquire mismatch)"
                );
                false
            }
        };
        if should_remove {
            if let Some(state) = watchers.remove(block_id) {
                tracing::debug!(%block_id, "unwatch: last consumer dropped, aborting watcher");
                state.task.abort();
            }
            drop(watchers); // release lock before registry.remove
            self.registry.remove(block_id);
        }
    }
}

// ── BuilderServices impl ────────────────────────────────────────────────

/// Default render expression for blocks whose watcher hasn't delivered data
/// yet.
pub fn loading_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "loading".to_string(),
        args: vec![],
    }
}

/// Default collection render expression for builder stubs that have no real
/// block data (`HeadlessBuilderServices` in holon-app, `StubBuilderServices`).
pub fn table_expr() -> RenderExpr {
    use holon_api::render_types::Arg;
    RenderExpr::FunctionCall {
        name: "table".to_string(),
        args: vec![Arg {
            name: Some("item_template".to_string()),
            value: RenderExpr::FunctionCall {
                name: "render_entity".to_string(),
                args: vec![],
            },
        }],
    }
}

impl ReactiveEngine {
    /// Access the FrontendSession (for operation dispatch in frontend-specific
    /// builders).
    pub fn session(&self) -> &Arc<FrontendSession> {
        &self.session
    }

    /// Access the tokio runtime handle.
    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.runtime_handle
    }

    /// SPIKE (Phase 1b): which occurrence of the focused block holds focus
    /// (`None` = canonical). The headless editor mirror keys its cursor map by
    /// `(block_id, occurrence)` so two display occurrences of one block get
    /// independent carets while edits still resolve to the canonical block.
    pub fn focused_occurrence(&self) -> Option<u32> {
        self.ui_state.focused_occurrence()
    }

    /// SPIKE (Phase 1b): set the focused occurrence (`None` = canonical).
    /// Additive to `set_focus`; leaves `focused_block` untouched. `pub` so the
    /// integration-tests crate's end-to-end occurrence test can drive it.
    pub fn set_focus_occurrence(&self, occ: Option<u32>) {
        self.ui_state.set_focus_occurrence(occ);
    }
}

impl BuilderServices for ReactiveEngine {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        self.services_slot
            .get()
            .expect(
                "services_slot not yet populated — frontend bootstrap must call \
                 services_slot.set(engine.clone()) before any lazy-widget interpretation",
            )
            .clone()
    }

    #[tracing::instrument(level = "debug", skip_all, fields(%id))]
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        let results = self.ensure_watching(id);
        results.snapshot()
    }

    /// Override the trait default to delegate to the inherent `snapshot`,
    /// which has thread-local cycle detection for `LiveBlock` resolution.
    /// Without this, `live_block(A) → live_block(B) → live_block(A)` (or
    /// any block that transitively embeds itself) blows the stack.
    fn snapshot_resolved(&self, block_id: &EntityUri) -> crate::view_model::ViewModel {
        self.snapshot(block_id)
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn resolve_profile(&self, row: &DataRow) -> Option<holon_api::RenderProfile> {
        self.session.resolve_row_profile(row)
    }

    fn profile_signal(&self) -> Mutable<Arc<holon_api::entity_profile::ProfileCache>> {
        self.session.profiles().profile_signal()
    }

    fn virtual_child_config(
        &self,
        entity_name: &str,
    ) -> Option<holon_api::entity_profile::VirtualChildConfig> {
        self.session.profiles().virtual_child_config(entity_name)
    }

    fn advice_children(&self, anchor: &EntityUri) -> Vec<Arc<DataRow>> {
        // Lazily bring the reactive weaver up on first read so a live session
        // keeps the sidecar fresh; the composed settle refreshes it explicitly.
        self.ensure_advice_weaver();
        self.advice_sidecar
            .lock()
            .unwrap()
            .get(anchor)
            .cloned()
            .unwrap_or_default()
    }

    fn entity_operations(
        &self,
        entity_name: &str,
    ) -> Vec<holon_api::render_types::OperationDescriptor> {
        self.session.profiles().operations_for(entity_name)
    }

    fn watch_query(
        &self,
        query: &str,
        lang: QueryLanguage,
        ctx: Option<crate::QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        let engine = self
            .session
            .query_engine()
            .ok_or_else(|| anyhow::anyhow!("no query engine in this (no-Turso) session"))?;
        let rt = self.runtime_handle.clone();
        std::thread::scope(|s| {
            s.spawn(|| rt.block_on(engine.watch_query(query, lang, HashMap::new(), ctx)))
                .join()
                .unwrap()
        })
    }

    fn query_engine(&self) -> Option<Arc<dyn holon_api::QueryEngine>> {
        self.session.query_engine()
    }

    fn widget_state(&self, id: &str) -> WidgetState {
        self.session.widget_state(id)
    }

    fn block_expanded_view(&self, target_id: &str) -> Option<bool> {
        self.ui_state.block_expanded_view(target_id)
    }

    fn widget_state_explicit(&self, id: &str) -> Option<WidgetState> {
        self.session.widget_state_explicit(id)
    }

    fn set_widget_open(&self, id: &str, open: bool) {
        self.session.set_widget_open(id, open);
    }

    fn set_preference(&self, key: &str, value: holon_api::Value) -> Result<()> {
        let pref_key = crate::preferences::PrefKey::new(key);
        let toml_value = crate::preferences::value_to_toml(&value);
        self.session.set_preference(&pref_key, toml_value)
    }

    fn dispatch_intent(&self, intent: crate::operations::OperationIntent) {
        if intent.entity_name == "preferences" && intent.op_name == "set" {
            if let (Some(key), Some(value)) = (
                intent.params.get("key").and_then(|v| v.as_string()),
                intent.params.get("value"),
            ) {
                let pref_key = crate::preferences::PrefKey::new(&key);
                let toml_value = crate::preferences::value_to_toml(value);
                // Fail-loud, not fatal: a failed preference persist (e.g. a
                // read-only config dir on Android) must be disclosed, never
                // abort the process. Callers with a UI seam (GPUI pref fields)
                // use the fallible `set_preference` trait method to also toast.
                if let Err(e) = self.session.set_preference(&pref_key, toml_value) {
                    self.session.error_tracker().record_error();
                    tracing::error!("Failed to persist preference {key}: {e:#}");
                }
            }
            return;
        }

        // Mirror `navigation.focus` into `UiState.focused_block` so
        // value-fn row providers (`focus_chain()`) see focus changes
        // without having to re-derive them from `navigation_cursor`.
        // The backend still writes the SQL tables; this just keeps the
        // frontend-side signal graph in sync.
        maybe_mirror_navigation_focus(&self.ui_state, &intent);
        maybe_clear_focus_on_delete(&self.ui_state, &intent);

        // Fire-and-forget execute. The result-hook projects a structural
        // op's focus result (`split_block`/`join_block`) straight onto the
        // in-memory authority — no Turso `editor_cursor` round-trip. Focus
        // handles are cloned in because the spawned task can't borrow `self`.
        let session = self.session.clone();
        let (focused_block, caret_seed) = self.ui_state.focus_handles();
        let entity_name = intent.entity_name.clone();
        let op_name = intent.op_name.clone();
        let params = intent.params;
        // End-to-end latency: start the interaction clock at the dispatch
        // entry point; `holon_api::latency_e2e` closes it when the target's
        // row lands in a LiveData mirror (stage="e2e").
        if let Some(target) = params.get("id").and_then(|v| v.as_string()) {
            holon_api::latency_e2e::interaction_dispatched(
                &op_name,
                target,
                holon_api::latency_e2e::write_seq_from_params(&params),
            );
        }
        self.runtime_handle.spawn(async move {
            match session
                .execute_operation(&entity_name, &op_name, params)
                .await
            {
                Ok(response) => {
                    apply_structural_focus(&focused_block, &caret_seed, &op_name, &response);
                }
                Err(e) => {
                    // Disclose the failed write: the UI already reflects the
                    // user's gesture, so a dropped error would silently look
                    // like success. The tracker is the PBT/monitoring seam.
                    session.error_tracker().record_error();
                    tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
                }
            }
        });
    }

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        ctx_params: HashMap<String, holon_api::Value>,
    ) {
        let matched = crate::operation_matcher::try_match_from_context(&op, &ctx_params);
        if matched.missing_params.is_empty() {
            self.dispatch_intent(crate::operations::OperationIntent {
                entity_name: op.entity_name.clone(),
                op_name: op.name.clone(),
                params: matched.resolved_params,
            });
            return;
        }
        // Multi-param activation (popup param-collection flow) is tracked as
        // follow-up work: extracting the CommandProvider param-collection
        // machinery out of `ViewEventHandler` and anchoring it to the
        // op_button site. For now fail loudly so it's visible.
        panic!(
            "present_op({}.{}): multi-param popup activation is not yet wired for op_button \
             sites; {} param(s) missing (follow-up to mobile-bar PR)",
            op.entity_name,
            op.name,
            matched.missing_params.len()
        );
    }

    // TODO: I've seen other `dispatch_intent...` methods. How do these relate to
    // each other? Anything to DRY?
    fn dispatch_intent_sync(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        // Preference sets run inline — no async execute_operation path.
        // TODO: The extra handling of "preferences" is not nice
        if intent.entity_name == "preferences" && intent.op_name == "set" {
            if let (Some(key), Some(value)) = (
                intent.params.get("key").and_then(|v| v.as_string()),
                intent.params.get("value"),
            ) {
                let pref_key = crate::preferences::PrefKey::new(&key);
                let toml_value = crate::preferences::value_to_toml(value);
                // Surface a persist failure to the sync caller instead of
                // aborting — this path awaits the result (MCP/tests/driver).
                return Box::pin(std::future::ready(
                    self.session.set_preference(&pref_key, toml_value),
                ));
            }
            return Box::pin(std::future::ready(Ok(())));
        }

        maybe_mirror_navigation_focus(&self.ui_state, &intent);
        maybe_clear_focus_on_delete(&self.ui_state, &intent);

        let session = self.session.clone();
        let (focused_block, caret_seed) = self.ui_state.focus_handles();
        Box::pin(async move {
            // Latency stage (dispatch->op-applied): a user action enters the
            // pipeline here. `block` is the entity the op targets; `action` the
            // op name (split_block, indent, outdent, cycle_state, ...). The push
            // pipeline (Loro commit -> projection -> CDC rows) runs downstream and
            // is measured by the `projection`/`rows` stages. Greppable via
            // target="holon_latency".
            let block = intent
                .params
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .unwrap_or_else(|| intent.entity_name.as_str().to_string());
            // End-to-end latency: start the interaction clock here;
            // `holon_api::latency_e2e` closes it when the target's row lands
            // in a LiveData mirror (stage="e2e").
            if let Some(target) = intent.params.get("id").and_then(|v| v.as_string()) {
                holon_api::latency_e2e::interaction_dispatched(
                    &intent.op_name,
                    target,
                    holon_api::latency_e2e::write_seq_from_params(&intent.params),
                );
            }
            let t_dispatch = std::time::Instant::now();
            let response = session
                .execute_operation(&intent.entity_name, &intent.op_name, intent.params)
                .await
                .with_context(|| {
                    format!(
                        "dispatch_intent_sync: {}.{} failed",
                        intent.entity_name, intent.op_name
                    )
                })?;
            tracing::debug!(
                target: "holon_latency",
                stage = "dispatch",
                action = %intent.op_name,
                block = %block,
                ms = t_dispatch.elapsed().as_millis() as u64,
                "holon_latency",
            );
            // Same in-process structural-focus projection as `dispatch_intent`.
            apply_structural_focus(&focused_block, &caret_seed, &intent.op_name, &response);
            Ok(())
        })
    }

    fn dispatch_intent_awaitable(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'static>> {
        maybe_mirror_navigation_focus(&self.ui_state, &intent);
        maybe_clear_focus_on_delete(&self.ui_state, &intent);
        let session = self.session.clone();
        let (focused_block, caret_seed) = self.ui_state.focus_handles();
        let entity_name = intent.entity_name.clone();
        let op_name = intent.op_name.clone();
        let params = intent.params;
        if let Some(target) = params.get("id").and_then(|v| v.as_string()) {
            holon_api::latency_e2e::interaction_dispatched(
                &op_name,
                target,
                holon_api::latency_e2e::write_seq_from_params(&params),
            );
        }
        Box::pin(async move {
            match session
                .execute_operation(&entity_name, &op_name, params)
                .await
            {
                Ok(response) => {
                    apply_structural_focus(&focused_block, &caret_seed, &op_name, &response);
                    Ok(())
                }
                Err(e) => {
                    // Disclose the failed write (same seam as `dispatch_intent`);
                    // the caller ALSO surfaces it as a visible toast.
                    session.error_tracker().record_error();
                    tracing::error!("dispatch_intent_awaitable: {entity_name}.{op_name}: {e:#}");
                    Err(e)
                }
            }
        })
    }

    fn follow_dangling_link(&self, target: String, region: String) {
        let session = self.session.clone();
        let (focused_block, main_nav) = self.ui_state.nav_focus_handles();
        // Capture focus at CLICK time to guard the last-writer race: the create
        // is async, so if the user navigates elsewhere during that window the
        // stale task must NOT stomp the newer focus. (Resolved-link clicks
        // mirror focus synchronously and have no such window.)
        let focus_at_click = focused_block.get_cloned();
        self.runtime_handle.spawn(async move {
            if let Err(e) = create_page_and_navigate(
                &session,
                &focused_block,
                &main_nav,
                &target,
                &region,
                focus_at_click,
            )
            .await
            {
                // Disclose the failed follow: the user clicked a dangling link
                // and nothing opened, so a dropped error would look like a dead
                // link. The tracker is the PBT/monitoring seam.
                session.error_tracker().record_error();
                tracing::error!("follow_dangling_link({target}) failed: {e:#}");
            }
        });
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn ui_state(&self, block_id: &EntityUri) -> HashMap<String, holon_api::Value> {
        self.ui_state.context_for(block_id)
    }

    fn viewport_snapshot(&self) -> Option<crate::render_context::AvailableSpace> {
        self.ui_state
            .viewport()
            .map(|vp| crate::render_context::AvailableSpace {
                width_px: vp.width_px,
                height_px: vp.height_px,
                width_physical_px: vp.width_px * vp.scale_factor,
                height_physical_px: vp.height_px * vp.scale_factor,
                scale_factor: vp.scale_factor,
            })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn key_bindings_snapshot(&self) -> std::collections::BTreeMap<String, holon_api::KeyChord> {
        self.key_bindings.lock_ref().clone()
    }

    fn focused_block(&self) -> Option<EntityUri> {
        self.ui_state.focused_block()
    }

    fn focused_block_mutable(&self) -> Option<Mutable<Option<EntityUri>>> {
        Some(self.ui_state.focused_block_mutable())
    }

    fn set_focus_with_caret(&self, block: EntityUri, offset: usize) {
        self.ui_state.set_focus_with_caret(block, offset);
    }

    fn peek_caret_seed(&self, block: &EntityUri) -> Option<usize> {
        self.ui_state.peek_caret_seed(block)
    }

    fn consume_caret_seed(&self, block: &EntityUri) {
        self.ui_state.consume_caret_seed(block);
    }

    fn provider_cache(&self) -> Option<Arc<crate::provider_cache::ProviderCache>> {
        Some(self.provider_cache.clone())
    }

    fn set_focus(&self, block_id: Option<EntityUri>) {
        // ALLOW(direct_focus_mutation): this IS the legitimate
        // BuilderServices::set_focus setter.
        self.ui_state.set_focus(block_id);
    }

    fn await_ready(
        &self,
        id: &EntityUri,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let results = self.ensure_watching(id);
        Box::pin(async move { results.wait_until_ready().await })
    }

    fn watch_block_signal(
        &self,
        block_id: &EntityUri,
    ) -> std::pin::Pin<
        Box<dyn futures_signals::signal::Signal<Item = crate::ReactiveViewModel> + Send>,
    > {
        self.watch_signal(block_id)
    }

    fn watch_live(
        &self,
        block_id: &EntityUri,
        services: Arc<dyn BuilderServices>,
    ) -> crate::LiveBlock {
        ReactiveEngine::watch_live(self, block_id, services)
    }

    fn watch_query_live(
        &self,
        query: String,
        lang: QueryLanguage,
        render_expr: holon_api::render_types::RenderExpr,
        query_context: Option<crate::QueryContext>,
        services: Arc<dyn BuilderServices>,
    ) -> (EntityUri, crate::LiveBlock) {
        ReactiveEngine::watch_query_live(self, query, lang, render_expr, query_context, services)
    }

    fn unwatch(&self, block_id: &EntityUri) {
        ReactiveEngine::unwatch(self, block_id);
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime_handle.clone()
    }

    fn list_templates(&self) -> Vec<crate::template_placement::TemplateChoice> {
        use crate::template_placement::TemplateChoice;
        let session = self.session.clone();
        let rt = self.runtime_handle.clone();
        // Bridge the async snapshot from a fresh thread — same pattern as
        // `watch_query` — so this stays callable from the GPUI main thread
        // whether or not it is itself a tokio worker.
        let snapshot = std::thread::scope(|s| {
            s.spawn(|| rt.block_on(session.block_query().snapshot()))
                .join()
                .unwrap()
        });
        let snapshot = match snapshot {
            Ok(snap) => snap,
            Err(e) => {
                // Fail loud in the log — an unreadable projection means the
                // picker silently offers nothing, which we disclose rather
                // than pretend is "no templates exist".
                tracing::error!("list_templates: block snapshot failed: {e}");
                return Vec::new();
            }
        };
        // Enumeration logic is a pure, unit-tested function driven over the
        // real block snapshot — its case-insensitive marker lookup is what
        // makes org-authored templates (uppercase `:TEMPLATE:`) appear.
        crate::template_placement::templates_from_blocks(snapshot.iter_blocks())
    }

    fn resolve_block(&self, id: &str) -> Option<holon_api::block::Block> {
        let session = self.session.clone();
        let rt = self.runtime_handle.clone();
        // Same fresh-thread snapshot bridge as `list_templates`.
        let snapshot = std::thread::scope(|s| {
            s.spawn(|| rt.block_on(session.block_query().snapshot()))
                .join()
                .unwrap()
        });
        let snapshot = match snapshot {
            Ok(snap) => snap,
            Err(e) => {
                tracing::error!("resolve_block('{id}'): block snapshot failed: {e}");
                return None;
            }
        };
        for block in snapshot.iter_blocks() {
            if block.id.as_str() == id {
                return Some(block.clone());
            }
        }
        None
    }

    fn search_link_candidates(
        &self,
        filter: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        let session = self.session.clone();
        let filter = filter.to_string();
        Box::pin(async move {
            session
                .query_engine()
                .ok_or_else(|| anyhow::anyhow!("no query engine in this (no-Turso) session"))?
                .search_link_candidates(&filter)
                .await
        })
    }

    fn editable_text(
        &self,
        block_id: &EntityUri,
        field: &str,
    ) -> anyhow::Result<crate::cell::Cell<String>> {
        let guard = self.block_cell_registry.lock().unwrap();
        match &*guard {
            Some(reg) => {
                use crate::cell::EntityCellRegistryExt;
                let reg_dyn: &dyn crate::cell::EntityCellRegistry = reg.as_ref();
                reg_dyn.live_field::<String>(block_id, field)
            }
            None => Err(anyhow::anyhow!(
                "editable_text not configured for this ReactiveEngine (BlockCellRegistry not \
                 wired)"
            )),
        }
    }
}

/// Zero-dependency stub for design galleries and standalone examples.
///
/// Returns empty/default data for everything. No BackendEngine, no database,
/// no DI — just enough to drive the shadow interpreter and produce ViewModels.
///
/// Owns a process-wide single-threaded tokio runtime for callers that need
/// a `runtime_handle()` (reactive shell spawn paths). Sync unit tests that
/// only call `interpret_pure` never touch the runtime and don't pay for it.
pub struct StubBuilderServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    rt_handle: tokio::runtime::Handle,
}

fn stub_runtime_handle() -> tokio::runtime::Handle {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        // wasm32 tokio only supports current_thread (no rt-multi-thread)
        #[cfg(target_arch = "wasm32")]
        let mut builder = tokio::runtime::Builder::new_current_thread();
        #[cfg(not(target_arch = "wasm32"))]
        let mut builder = {
            let mut b = tokio::runtime::Builder::new_multi_thread();
            b.worker_threads(1).thread_name("stub-builder-services");
            b
        };
        builder.enable_all().build().expect("stub runtime build")
    })
    .handle()
    .clone()
}

impl StubBuilderServices {
    pub fn new() -> Self {
        let rt_handle =
            tokio::runtime::Handle::try_current().unwrap_or_else(|_| stub_runtime_handle());
        Self {
            interpreter: Arc::new(crate::shadow_builders::build_shadow_interpreter()),
            rt_handle,
        }
    }

    pub fn with_handle(rt_handle: tokio::runtime::Handle) -> Self {
        Self {
            interpreter: Arc::new(crate::shadow_builders::build_shadow_interpreter()),
            rt_handle,
        }
    }
}

impl Default for StubBuilderServices {
    fn default() -> Self {
        Self::new()
    }
}

impl BuilderServices for StubBuilderServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (table_expr(), vec![])
    }

    fn resolve_profile(&self, _: &DataRow) -> Option<holon_api::RenderProfile> {
        None
    }

    fn watch_query(
        &self,
        _: &str,
        _: QueryLanguage,
        _: Option<crate::QueryContext>,
    ) -> Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("StubBuilderServices does not support live queries")
    }

    fn widget_state(&self, _: &str) -> WidgetState {
        WidgetState::default()
    }

    fn set_widget_open(&self, _: &str, _: bool) {
        // Stub services don't persist widget state.
    }

    fn dispatch_intent(&self, intent: crate::operations::OperationIntent) {
        tracing::info!(
            "StubBuilderServices.dispatch_intent({}.{}) — no-op in stub mode",
            intent.entity_name,
            intent.op_name
        );
    }

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _: HashMap<String, holon_api::Value>,
    ) {
        panic!(
            "StubBuilderServices::present_op({}.{}) — op_button must not be reached under a stub \
             services instance. If a gallery/example renders the mobile action bar it should swap \
             in a real ReactiveEngine, not route through the stub.",
            op.entity_name, op.name
        );
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }

    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        Box::pin(async {
            anyhow::bail!("StubBuilderServices does not support search_link_candidates")
        })
    }
}

/// Stable watcher-registry key for a live query: source query + language +
/// context ids. Including the context fixes a latent collision where two
/// nodes with the same query but different contexts shared one watcher.
fn hash_query(query: &str, lang: QueryLanguage, ctx: &Option<crate::QueryContext>) -> u64 {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut hasher);
    format!("{lang:?}").hash(&mut hasher);
    if let Some(ctx) = ctx {
        format!("{:?}", ctx.current_block_id).hash(&mut hasher);
        format!("{:?}", ctx.context_parent_id).hash(&mut hasher);
        ctx.context_path_prefix.hash(&mut hasher);
    }
    hasher.finish()
}

// ── DI integration ──────────────────────────────────────────────────────

/// Slot for BuilderServices, stored in DI.
///
/// Breaks the circular dependency: interpret_fn needs BuilderServices,
/// but BuilderServices IS the ReactiveEngine which needs interpret_fn.
/// The slot is registered in DI, and populated after ReactiveEngine creation.
pub struct BuilderServicesSlot(pub Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>>);

/// Newtype wrapper for the render interpreter function, stored in DI.
///
/// Registered via `set_render_interpreter()`. Resolved by the ReactiveEngine
/// factory.
pub struct RenderInterpreterFn(
    pub Arc<dyn Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync>,
);

/// Extension trait for registering the render interpreter in DI.
///
/// The render interpreter is the one frontend-specific dependency. Everything
/// else is resolved from DI automatically.
pub trait RenderInterpreterInjectorExt {
    /// Register a render interpreter function in DI.
    ///
    /// Call this before resolving `ReactiveEngine`.
    fn set_render_interpreter(
        &self,
        interpret_fn: impl Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    );
}

impl RenderInterpreterInjectorExt for Injector {
    fn set_render_interpreter(
        &self,
        interpret_fn: impl Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    ) {
        let f: Arc<dyn Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync> =
            Arc::new(interpret_fn);
        let shared = Shared::new(RenderInterpreterFn(f));
        self.provide::<RenderInterpreterFn>(Provider::root(move |_| shared.clone()));
    }
}

/// Mirror a `navigation.focus` intent into `UiState.focused_block`.
///
/// The backend `NavigationProvider::focus` op writes `navigation_cursor`
/// + `navigation_history` in SQL, but there is no CDC path back into the
/// frontend's `UiState` — so value-fn providers like `focus_chain()`
/// would stay empty even after navigation. This side-channel keeps them
/// in sync. Called from both `dispatch_intent` and `dispatch_intent_sync`.
fn maybe_mirror_navigation_focus(ui_state: &UiState, intent: &crate::operations::OperationIntent) {
    if intent.entity_name != "navigation" {
        return;
    }
    match NavigationOp::from_str(&intent.op_name) {
        // `navigation.focus` (sidebar / page navigation) has no CDC path back
        // into `UiState`, so mirror it here. (Editor focus is no longer a
        // dispatched op — clicks call `set_focus` directly and split/join set
        // it from their op result; see ADR 0010.)
        Ok(NavigationOp::Focus) => {
            let block_id = intent.params.get("block_id").and_then(|v| v.as_string());
            // End-to-end latency: navigation is a first-class interaction.
            // Start the interaction clock keyed on the focused block; the
            // `latency_e2e` correlator closes it (stage="e2e", action="navigate")
            // when the page's rows land in a LiveData mirror — a child row
            // carries `parent_id = block_id`. Tokenless (reads carry no
            // `write_seq`).
            if let Some(target) = block_id {
                holon_api::latency_e2e::interaction_dispatched("navigate", target, None);
            }
            // ALLOW(entity_uri_from_raw): block_id from intent.params Value map
            // (operation-intent ingest)
            let block_id = block_id.map(EntityUri::from_raw);
            // ALLOW(direct_focus_mutation): mirror of navigation.focus into UiState for
            // value-fn graph; intentional, see surrounding comment.
            ui_state.set_focus(block_id);
            // A page navigation into the main region resets main-panel scroll
            // (LogSeq parity). Region-scoped so a right-sidebar pin (region=right)
            // leaves the main scroll alone. An absent region defaults to main
            // (the sidebar/journal nav actions all target region=main).
            let region = intent
                .params
                .get("region")
                .and_then(|v| v.as_string())
                .unwrap_or("main");
            if region == "main" {
                ui_state.bump_main_nav();
            }
        }
        // ALLOW(direct_focus_mutation): mirror of navigation.go_home into UiState for value-fn
        // graph.
        Ok(NavigationOp::GoHome) => {
            ui_state.set_focus(None);
            ui_state.bump_main_nav();
        }
        // `focus_pin` / `close` / `go_back` / `go_forward` would require reading
        // `navigation_history` to know the target — leave them alone until the
        // backend grows a synchronous "current focus" accessor. `Err` is a
        // non-navigation op (the `entity_name` guard above keeps it out here).
        Ok(
            NavigationOp::FocusPin
            | NavigationOp::Close
            | NavigationOp::GoBack
            | NavigationOp::GoForward,
        )
        | Err(_) => {}
    }
}

/// Clear focus when the focused block is being deleted.
///
/// Editor focus is in-memory UI state (ADR 0010), so the signal owner must keep
/// it consistent with the store — the `current_editor_focus` matview used to do
/// this implicitly via IVM recomputation. A dangling focus on a deleted block
/// would mis-route chord ops and keep the render-variant switch pointed at a
/// gone block. Mirrors the PBT reference model's `clear_focus_if_deleted`.
/// Called from both `dispatch_intent` and `dispatch_intent_sync`.
fn maybe_clear_focus_on_delete(ui_state: &UiState, intent: &crate::operations::OperationIntent) {
    if intent.op_name != "delete" {
        return;
    }
    let Some(id) = intent.params.get("id").and_then(|v| v.as_string()) else {
        return;
    };
    // ALLOW(entity_uri_from_raw): id from intent.params Value map (operation-intent
    // ingest)
    let deleted = EntityUri::from_raw(id);
    if ui_state.focused_block().as_ref() == Some(&deleted) {
        // ALLOW(direct_focus_mutation): clear focus of a deleted block, mirroring the
        // reference model.
        ui_state.set_focus(None);
    }
}

/// Extract the post-op focus target from a structural focus-mover's response.
///
/// `split_block` / `join_block` return `Value::Object{block_id, cursor_offset}`
/// (split → 0, join → join boundary). This replaces the old backend
/// `editor_focus` follow-up: instead of routing focus back through the Turso
/// `current_editor_focus` CDC stream, the frontend reads it straight off the
/// operation result and moves the in-memory authority in-process.
fn structural_focus_target(
    op_name: &str,
    response: &Option<holon_api::Value>,
) -> Option<(EntityUri, usize)> {
    if op_name != "split_block" && op_name != "join_block" {
        return None;
    }
    let Some(holon_api::Value::Object(map)) = response.as_ref() else {
        return None;
    };
    let block_id = map.get("block_id").and_then(|v| v.as_string())?;
    let offset = map
        .get("cursor_offset")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    // ALLOW(entity_uri_from_raw): block_id from a structural op response
    // (operation-result ingest boundary)
    Some((EntityUri::from_raw(&block_id), offset.max(0) as usize))
}

/// Apply a structural op's focus result to the in-memory authority + caret
/// seed. Operates on cloned `Mutable` handles so it can run inside the
/// spawned dispatch task (which can't borrow `&UiState`).
fn apply_structural_focus(
    focused_block: &Mutable<Option<EntityUri>>,
    caret_seed: &Mutable<Option<(EntityUri, usize)>>,
    op_name: &str,
    response: &Option<holon_api::Value>,
) {
    if let Some((block, offset)) = structural_focus_target(op_name, response) {
        caret_seed.set(Some((block.clone(), offset)));
        if focused_block.get_cloned().as_ref() != Some(&block) {
            focused_block.set(Some(block));
        }
    }
}

/// Create the page chain for a *dangling* wiki-link `target`, then navigate
/// `region` to the freshly-created leaf page. Factored out of
/// [`ReactiveEngine::follow_dangling_link`] so the create→navigate chain —
/// which depends on the fresh leaf-page id carried in the create op's response
/// — can run inside a spawned task over cloned `Mutable` handles, and so the
/// response→nav-target decision is unit-testable via
/// [`dangling_link_nav_target`].
///
/// Navigation is mirrored into `UiState` (so value-fn providers like
/// `focus_chain` observe it) and then persisted through the backend
/// `navigation.focus` op, exactly as `maybe_mirror_navigation_focus` +
/// `NavigationProvider` do for an ordinary click on a resolved link.
async fn create_page_and_navigate(
    session: &Arc<FrontendSession>,
    focused_block: &Mutable<Option<EntityUri>>,
    main_nav: &Mutable<u64>,
    target: &str,
    region: &str,
    focus_at_click: Option<EntityUri>,
) -> Result<()> {
    let create_params = [(
        "target".to_string(),
        holon_api::Value::String(target.to_string()),
    )]
    .into_iter()
    .collect();
    let response = session
        .execute_operation(
            &holon_api::EntityName::new("block"),
            "create_page_from_link",
            create_params,
        )
        .await
        .with_context(|| format!("create_page_from_link({target})"))?;

    let (leaf, reset_scroll) = dangling_link_nav_target(&response, region)
        .with_context(|| format!("create_page_from_link({target}) response"))?;

    // Last-writer guard: the page CREATE always happened (and the healed link
    // makes the next click resolve), but only navigate if no newer navigation
    // landed during the async create window — otherwise a stale task would
    // stomp the user's newer focus in both UiState and persisted nav-history.
    let focus_now = focused_block.get_cloned();
    if dangling_nav_superseded(&focus_at_click, &focus_now) {
        tracing::info!(
            "dangling-link navigation superseded by newer navigation \
             (target={target}); page created, skipping focus"
        );
        return Ok(());
    }

    // Mirror the navigation into UiState before persisting it, matching the
    // synchronous click-time mirror of an ordinary `navigation.focus`.
    if focus_now.as_ref() != Some(&leaf) {
        focused_block.set(Some(leaf.clone()));
    }
    if reset_scroll {
        main_nav.set(main_nav.get() + 1);
    }

    let nav_params = [
        (
            "region".to_string(),
            holon_api::Value::String(region.to_string()),
        ),
        (
            "block_id".to_string(),
            holon_api::Value::String(leaf.to_string()),
        ),
    ]
    .into_iter()
    .collect();
    session
        .execute_operation(
            &holon_api::EntityName::new("navigation"),
            "focus",
            nav_params,
        )
        .await
        .context("navigation.focus after create_page_from_link")?;
    Ok(())
}

/// Decide the post-create navigation target from a `create_page_from_link`
/// response. The op returns the fresh leaf-page id as `Value::String`;
/// navigation focuses that page and, for `region == "main"`, resets the
/// main-panel scroll. Any other response shape is a fail-loud contract
/// violation (the op's response type changed out from under this caller).
fn dangling_link_nav_target(
    response: &Option<holon_api::Value>,
    region: &str,
) -> Result<(EntityUri, bool)> {
    match response {
        Some(holon_api::Value::String(leaf_id)) => {
            // Operation-result ingest boundary (as in structural_focus_target).
            // ALLOW(entity_uri_from_raw): leaf page id from create_page_from_link response
            let leaf = EntityUri::from_raw(leaf_id);
            Ok((leaf, region == "main"))
        }
        other => Err(anyhow::anyhow!(
            "create_page_from_link must return the leaf page id as Value::String, got: {other:?}"
        )),
    }
}

/// Whether a pending dangling-link navigation has been superseded: `true` when
/// focus moved between the click that started the async page-create and the
/// moment the create completed. A stale task must not stomp the newer focus —
/// the page create still stands, only the focus move is skipped.
fn dangling_nav_superseded(
    focus_at_click: &Option<EntityUri>,
    focus_now: &Option<EntityUri>,
) -> bool {
    focus_at_click != focus_now
}

/// Dispatch `intents` as ONE ordered fire-and-forget chain: a single spawned
/// task awaits each via `dispatch_intent_sync` in sequence, aborting on the
/// first failure (loudly — the remaining intents must not run against state
/// the failed one was supposed to establish).
///
/// This is the dispatch primitive for "structural ops are commit points"
/// (docs/Architecture/UI.md): an editor flushing pending text before a
/// split/join MUST order the flush before the structural op. Two
/// `dispatch_intent` calls each spawn their own task and can reorder; this
/// cannot. UI callers (GPUI handlers) stay non-blocking — the chain runs on
/// the services' runtime.
pub fn dispatch_intent_chain(
    services: &Arc<dyn BuilderServices>,
    intents: Vec<crate::operations::OperationIntent>,
) {
    let services = services.clone();
    services.clone().runtime_handle().spawn(async move {
        for intent in intents {
            let label = format!("{}.{}", intent.entity_name, intent.op_name);
            if let Err(e) = services.dispatch_intent_sync(intent).await {
                tracing::error!(
                    "dispatch_intent_chain: {label} failed — aborting remaining intents: {e:#}"
                );
                return;
            }
        }
    });
}

/// Pure ViewModel construction: render expression + data rows + services →
/// ViewModel tree.
///
/// Thin free-function wrapper that forwards to
/// `services.interpret_with_source`. Retained so external callers (PBT
/// reference model, widget gallery, tests) can keep their existing call-site
/// shape.
#[tracing::instrument(level = "debug", skip_all)]
pub fn interpret_pure(
    expr: &RenderExpr,
    rows: &[Arc<DataRow>],
    services: &dyn BuilderServices,
) -> ReactiveViewModel {
    let ctx = RenderContext {
        data_rows: rows.to_vec().into(),
        available_space: services.viewport_snapshot(),
        ..Default::default()
    };
    services.interpret(expr, &ctx)
}

/// Build the default interpret function for the ReactiveEngine.
///
/// Uses a `OnceLock<Arc<dyn BuilderServices>>` to break the circular
/// dependency: engine needs interpret_fn, interpret_fn needs services, services
/// IS the engine. The services are set after engine construction.
///
/// Shared by all frontends (GPUI, PBT, etc.) — the shadow interpreter is
/// platform-agnostic and produces `ReactiveViewModel`, not UI widgets.
pub fn make_interpret_fn(
    services_slot: Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>>,
) -> impl Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync {
    move |expr, rows| {
        let services = services_slot
            .get()
            .expect("BuilderServices not yet initialized")
            .clone();
        interpret_pure(expr, rows, &*services)
    }
}

// ── LiveBlock ───────────────────────────────────────────────────────────

/// A live watched block with per-row collection reactivity.
///
/// `tree` is the current `ReactiveViewModel`. Collection children within
/// it have `MutableVec`s that are updated in-place by background tasks
/// when individual rows change. `structural_changes` emits only when the
/// render expression changes — requiring a full rebuild (get a new
/// `LiveBlock`).
pub struct LiveBlock {
    pub tree: ReactiveViewModel,
    /// Emits a new tree when the render expression changes (structural
    /// rebuild). Data-only changes do NOT emit — they update the existing
    /// tree in-place.
    pub structural_changes: Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>>,
    /// RAII pin on the underlying watcher — dropping it (with the LiveBlock,
    /// or after `take()`ing it out) releases the engine's watcher when this
    /// was the last consumer. `None` only for stub/test constructors that
    /// don't own a real engine watcher (e.g. layout-testing fixtures).
    pub watch_guard: Option<WatchGuard>,
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use holon_api::Value;

    use super::*;

    fn focus_intent(op: &str, id: &str) -> crate::operations::OperationIntent {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        crate::operations::OperationIntent::new("block".into(), op.to_string(), params)
    }

    #[test]
    fn regeneration_initial_snapshot_drops_rows_the_new_query_no_longer_returns() {
        use holon_api::streaming::Batch;
        use holon_api::streaming::BatchMetadata;
        use holon_api::streaming::Change;
        use holon_api::streaming::ChangeOrigin;
        use holon_api::streaming::UiEvent;
        use holon_api::streaming::WithMetadata;

        fn row(id: &str, content: &str) -> HashMap<String, Value> {
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("content".to_string(), Value::String(content.to_string())),
            ])
        }
        fn data_event(generation: u64, rows: Vec<HashMap<String, Value>>) -> UiEvent {
            UiEvent::Data {
                batch: WithMetadata {
                    inner: Batch {
                        items: rows
                            .into_iter()
                            .map(|data| Change::Created {
                                data,
                                origin: ChangeOrigin::Local {
                                    operation_id: None,
                                    trace_id: None,
                                },
                            })
                            .collect(),
                    },
                    metadata: BatchMetadata {
                        relation_name: "t".into(),
                        trace_context: None,
                        sync_token: None,
                        seq: 0,
                    },
                },
                generation,
            }
        }
        fn structure_event(generation: u64) -> UiEvent {
            UiEvent::Structure {
                render_expr: RenderExpr::Literal {
                    value: holon_api::Value::String(format!("expr-{generation}")),
                },
                candidates: Vec::new(),
                generation,
            }
        }
        fn ids(rqr: &ReactiveRenderedRows) -> Vec<String> {
            let (_, rows) = rqr.snapshot();
            let mut v: Vec<String> = rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|x| x.as_string()).map(String::from))
                .collect();
            v.sort();
            v
        }

        let rqr = ReactiveRenderedRows::new();
        // Generation 1: query returns A + B.
        rqr.apply_event(structure_event(1));
        rqr.apply_event(data_event(
            1,
            vec![row("block:a", "1"), row("block:b", "1")],
        ));
        assert_eq!(ids(&rqr), vec!["block:a", "block:b"]);

        // Generation 2 (query changed, e.g. layout swap): snapshot returns B + C.
        // A must be dropped — it would otherwise live on as a ghost row.
        rqr.apply_event(structure_event(2));
        rqr.apply_event(data_event(
            2,
            vec![row("block:b", "2"), row("block:c", "2")],
        ));
        assert_eq!(ids(&rqr), vec!["block:b", "block:c"]);

        // A second Data batch in the SAME generation is an incremental CDC
        // delta, NOT a snapshot — it must not drop unmentioned rows.
        rqr.apply_event(data_event(2, vec![row("block:d", "2")]));
        assert_eq!(ids(&rqr), vec!["block:b", "block:c", "block:d"]);

        // Stale-generation data is still discarded.
        rqr.apply_event(data_event(1, vec![row("block:z", "stale")]));
        assert_eq!(ids(&rqr), vec!["block:b", "block:c", "block:d"]);
    }

    /// Regression (dogfood 2026-07-10, crash chain: focus `block:journals` →
    /// render the journals day-list collection): a Created row that carries NO
    /// entity-shaped `id` (the day-list `SELECT date('now') AS name` row
    /// `{_rowid:1, name:2026-07-10}`) used to hit
    /// `data_row_entity_uri(..).expect("Created event must have 'id' column")`
    /// and PANIC the tokio render worker (`reactive.rs:485`), taking down the
    /// page. It must now degrade: keyed on `_rowid` under a `degraded:` scheme,
    /// stored and rendered (mirroring the profile resolver's visible degrade),
    /// and it must survive the first-batch `retain_keys` sweep.
    #[test]
    fn id_less_created_row_degrades_instead_of_panicking_the_worker() {
        use holon_api::streaming::Batch;
        use holon_api::streaming::BatchMetadata;
        use holon_api::streaming::Change;
        use holon_api::streaming::ChangeOrigin;
        use holon_api::streaming::UiEvent;
        use holon_api::streaming::WithMetadata;

        let rqr = ReactiveRenderedRows::new();
        rqr.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::Literal {
                value: Value::String("day-list".into()),
            },
            candidates: Vec::new(),
            generation: 1,
        });

        // The exact id-less row from the crash log.
        let id_less: DataRow = HashMap::from([
            ("_rowid".to_string(), Value::Integer(1)),
            ("name".to_string(), Value::String("2026-07-10".to_string())),
        ]);
        rqr.apply_event(UiEvent::Data {
            batch: WithMetadata {
                inner: Batch {
                    items: vec![Change::Created {
                        data: id_less,
                        origin: ChangeOrigin::Local {
                            operation_id: None,
                            trace_id: None,
                        },
                    }],
                },
                metadata: BatchMetadata {
                    relation_name: "journals".into(),
                    trace_context: None,
                    sync_token: None,
                    seq: 0,
                },
            },
            generation: 1,
        });

        // No panic, and the degraded row is retained (survives the first-batch
        // retain sweep) and still carries its content for display.
        let (_, rows) = rqr.snapshot();
        assert_eq!(rows.len(), 1, "id-less row must be stored, not dropped");
        assert_eq!(
            rows[0].get("name").and_then(|v| v.as_string()),
            Some("2026-07-10"),
            "degraded row must keep its content for visible rendering",
        );
    }

    #[test]
    fn structural_focus_target_parses_split_join_response() {
        let resp = Some(Value::Object(HashMap::from([
            ("block_id".to_string(), Value::String("block:abc".into())),
            ("cursor_offset".to_string(), Value::Integer(7)),
        ])));
        let (uri, off) = structural_focus_target("split_block", &resp).expect("parses");
        assert_eq!(uri, EntityUri::block("abc"));
        assert_eq!(off, 7);
        // Non-structural ops are ignored.
        assert!(structural_focus_target("set_field", &resp).is_none());
        // Negative offsets clamp to 0 (never panics on `as usize`).
        let neg = Some(Value::Object(HashMap::from([
            ("block_id".to_string(), Value::String("block:x".into())),
            ("cursor_offset".to_string(), Value::Integer(-3)),
        ])));
        assert_eq!(structural_focus_target("join_block", &neg).unwrap().1, 0);
    }

    #[test]
    fn caret_seed_peek_is_non_destructive_and_aged_by_set_focus() {
        let ui = UiState::new();
        let a = EntityUri::block("a");
        let b = EntityUri::block("b");
        ui.set_focus_with_caret(a.clone(), 5);
        // Peek twice: same answer both times (non-destructive).
        assert_eq!(ui.peek_caret_seed(&a), Some(5));
        assert_eq!(ui.peek_caret_seed(&a), Some(5));
        // Re-focusing the same block keeps the seed.
        ui.set_focus(Some(a.clone()));
        assert_eq!(ui.peek_caret_seed(&a), Some(5));
        // Focusing a different block ages the seed out.
        ui.set_focus(Some(b.clone()));
        assert_eq!(ui.peek_caret_seed(&a), None);
        assert_eq!(ui.peek_caret_seed(&b), None);
    }

    /// Regression (BugFunnel 2026-07-11 row 80): the click-places-caret
    /// contract broke after a split→join sequence. The structural op armed a
    /// one-shot caret seed (split → 0, join → boundary); the mounting editor
    /// applied it, but the seed was NEVER consumed — it lingered in
    /// `pending_caret_seed`, aged only by a focus MOVE to a *different* block.
    /// After a "failed click elsewhere" (window blur without a focused-block
    /// change) the seed stayed armed for the same block, and the next click on
    /// it re-applied the stale offset (0 after an undone/redone split), yanking
    /// the caret to 0 so typing PREPENDED.
    ///
    /// `consume_caret_seed` makes the seed strictly single-use: once the mount
    /// applies it, `peek` returns `None` even while focus stays put, so a later
    /// click derives its caret from the current buffer (click position / end),
    /// never the stale op-follow-up offset. This models the exact sequence GPUI
    /// `grab_focus_and_seed_caret` drives at the seed-lifecycle seam (the caret
    /// APPLY itself needs a GPUI window; the lifecycle is the bug).
    #[test]
    fn caret_seed_is_single_use_so_a_later_click_is_not_yanked() {
        let ui = UiState::new();
        let a = EntityUri::block("a");
        let b = EntityUri::block("b");

        // split(a) → new block b focused, seed (b, 0).
        ui.set_focus_with_caret(b.clone(), 0);
        assert_eq!(ui.peek_caret_seed(&b), Some(0));

        // join(b) → merges back into a at the join boundary; seed (a, 5).
        ui.set_focus_with_caret(a.clone(), 5);
        assert_eq!(ui.peek_caret_seed(&a), Some(5));
        // The split's (b, 0) seed was overwritten by the join, not left behind.
        assert_eq!(ui.peek_caret_seed(&b), None);

        // The merged editor mounts / gains focus and APPLIES the seed once.
        // The mount consumes it — this is the fix.
        assert_eq!(ui.peek_caret_seed(&a), Some(5));
        ui.consume_caret_seed(&a);

        // "Failed click elsewhere": window focus leaves the editor but the
        // focused-block signal stays on `a` (no `set_focus`), so the OLD aging
        // path would NOT have cleared the seed. With single-use consumption it
        // is already gone.
        assert_eq!(ui.focused_block(), Some(a.clone()));
        assert_eq!(
            ui.peek_caret_seed(&a),
            None,
            "consumed seed must not survive to be re-applied on a re-click"
        );

        // Subsequent fresh click on `a`: the editor sees no seed and derives
        // the caret from the click / buffer end — no yank to 0, no prepend.
        ui.set_focus(Some(a.clone()));
        assert_eq!(ui.peek_caret_seed(&a), None);
    }

    /// Consuming a seed armed for one block must not clear a seed armed for
    /// another — each block's mount consumes only its own.
    #[test]
    fn consume_caret_seed_is_scoped_to_its_block() {
        let ui = UiState::new();
        let a = EntityUri::block("a");
        let b = EntityUri::block("b");
        ui.set_focus_with_caret(a.clone(), 3);
        // A no-op for a block that doesn't own the seed.
        ui.consume_caret_seed(&b);
        assert_eq!(ui.peek_caret_seed(&a), Some(3));
        // Consuming the owner clears it.
        ui.consume_caret_seed(&a);
        assert_eq!(ui.peek_caret_seed(&a), None);
    }

    #[test]
    fn clear_focus_on_delete_only_clears_the_focused_block() {
        let ui = UiState::new();
        let a = EntityUri::block("a");
        ui.set_focus(Some(a.clone()));
        // Deleting a different block leaves focus intact.
        maybe_clear_focus_on_delete(&ui, &focus_intent("delete", "block:b"));
        assert_eq!(ui.focused_block(), Some(a.clone()));
        // A non-delete op never clears.
        maybe_clear_focus_on_delete(&ui, &focus_intent("set_field", "block:a"));
        assert_eq!(ui.focused_block(), Some(a.clone()));
        // Deleting the focused block clears it.
        maybe_clear_focus_on_delete(&ui, &focus_intent("delete", "block:a"));
        assert_eq!(ui.focused_block(), None);
    }

    /// Build a `navigation.focus` intent targeting `region` at page `block_id`.
    fn nav_focus_intent(region: &str, block_id: &str) -> crate::operations::OperationIntent {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::String(region.to_string()));
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));
        crate::operations::OperationIntent::new("navigation".into(), "focus".to_string(), params)
    }

    /// The real navigation wiring must start the end-to-end latency clock:
    /// driving a `navigation.focus` intent through
    /// `maybe_mirror_navigation_focus` (the path both `dispatch_intent` and
    /// `dispatch_intent_sync` take) must enroll a `navigate` interaction
    /// for the focused `block_id` in the process-global `latency_e2e`
    /// registry. A unique target keeps this hermetic under the parallel
    /// runner. Guards against a regression where nav ops carry `block_id`
    /// (not `id`) and so slip past the clock.
    #[test]
    fn navigation_focus_starts_latency_clock() {
        let ui = UiState::new();
        let target = "block:nav-latency-clock-probe";
        maybe_mirror_navigation_focus(&ui, &nav_focus_intent("main", target));
        assert!(
            holon_api::latency_e2e::pending_targets()
                .iter()
                .any(|t| t == target),
            "navigation.focus must enroll a latency interaction for its block_id"
        );
    }

    /// The main-panel scroll-reset signal (dogfood #5 row 146): a page
    /// navigation into the `main` region bumps `main_nav_generation`; a
    /// right-sidebar pin (region=right) leaves it alone so the main scroll is
    /// preserved. `go_home` bumps it (returns to the home page).
    #[test]
    fn main_nav_generation_bumps_only_on_main_region_navigation() {
        let ui = UiState::new();
        assert_eq!(ui.main_nav_generation(), 0);

        // Navigating a page into the main region bumps the counter.
        maybe_mirror_navigation_focus(&ui, &nav_focus_intent("main", "block:page-a"));
        assert_eq!(ui.main_nav_generation(), 1);

        // Every subsequent main navigation advances it.
        maybe_mirror_navigation_focus(&ui, &nav_focus_intent("main", "block:page-b"));
        assert_eq!(ui.main_nav_generation(), 2);

        // A right-sidebar pin must NOT reset the main panel's scroll.
        maybe_mirror_navigation_focus(&ui, &nav_focus_intent("right", "block:page-c"));
        assert_eq!(ui.main_nav_generation(), 2);

        // go_home returns to the main region's home page → bump.
        let go_home = crate::operations::OperationIntent::new(
            "navigation".into(),
            "go_home".to_string(),
            HashMap::new(),
        );
        maybe_mirror_navigation_focus(&ui, &go_home);
        assert_eq!(ui.main_nav_generation(), 3);
    }

    fn make_row(id: &str, content: &str) -> DataRow {
        let mut row = DataRow::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String(content.to_string()));
        row
    }

    /// Test helper: enrich a DataRow (with no-op computed fields).
    fn enriched(row: DataRow) -> EnrichedRow {
        EnrichedRow::from_raw(row, |_| HashMap::new())
    }

    fn test_interpret(expr: &RenderExpr, rows: &[Arc<DataRow>]) -> ReactiveViewModel {
        let name = match expr {
            RenderExpr::FunctionCall { name, .. } => name.clone(),
            _ => "other".to_string(),
        };
        let mut m = HashMap::new();
        m.insert(
            "debug".to_string(),
            Value::String(format!("{}:{}", name, rows.len())),
        );
        ReactiveViewModel::from_widget("empty", HashMap::new()).with_entity(Arc::new(m))
    }

    fn remote_origin() -> holon_api::ChangeOrigin {
        holon_api::ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        }
    }

    fn debug_tag(vm: &ReactiveViewModel) -> String {
        let entity = vm.entity();
        entity
            .get("debug")
            .unwrap()
            .as_string()
            .unwrap()
            .to_string()
    }

    macro_rules! poll_signal {
        ($signal:expr) => {{
            use futures::StreamExt;
            use futures_signals::signal::SignalExt;
            let stream = $signal.to_stream();
            futures::pin_mut!(stream);
            stream.next().await.unwrap()
        }};
    }

    #[tokio::test]
    async fn initial_state_is_loading() {
        let rq = ReactiveRenderedRows::new();
        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        let debug = debug_tag(&vm);
        assert_eq!(debug, "loading:0");
    }

    #[tokio::test]
    async fn structure_event_sets_render_expr() {
        let rq = ReactiveRenderedRows::new();
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 1,
        });

        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        assert_eq!(debug_tag(&vm), "table:0");
    }

    #[tokio::test]
    async fn data_event_adds_rows() {
        let rq = ReactiveRenderedRows::new();
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 1,
        });
        rq.apply_event(UiEvent::Data {
            batch: holon_api::streaming::BatchMapChangeWithMetadata {
                inner: holon_api::streaming::Batch {
                    items: vec![holon_api::Change::Created {
                        data: make_row("r1", "hello"),
                        origin: remote_origin(),
                    }],
                },
                metadata: holon_api::streaming::BatchMetadata {
                    relation_name: String::new(),
                    trace_context: None,
                    sync_token: None,
                    seq: 0,
                },
            },
            generation: 1,
        });

        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        assert_eq!(debug_tag(&vm), "table:1");
    }

    #[tokio::test]
    async fn stale_data_ignored() {
        let rq = ReactiveRenderedRows::new();
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 2,
        });
        // Stale generation=1
        rq.apply_event(UiEvent::Data {
            batch: holon_api::streaming::BatchMapChangeWithMetadata {
                inner: holon_api::streaming::Batch {
                    items: vec![holon_api::Change::Created {
                        data: make_row("r1", "stale"),
                        origin: remote_origin(),
                    }],
                },
                metadata: holon_api::streaming::BatchMetadata {
                    relation_name: String::new(),
                    trace_context: None,
                    sync_token: None,
                    seq: 0,
                },
            },
            generation: 1,
        });

        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        assert_eq!(debug_tag(&vm), "table:0");
    }

    #[tokio::test]
    async fn structure_does_not_clear_data() {
        let rq = ReactiveRenderedRows::new();
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 1,
        });
        rq.apply_event(UiEvent::Data {
            batch: holon_api::streaming::BatchMapChangeWithMetadata {
                inner: holon_api::streaming::Batch {
                    items: vec![holon_api::Change::Created {
                        data: make_row("r1", "hello"),
                        origin: remote_origin(),
                    }],
                },
                metadata: holon_api::streaming::BatchMetadata {
                    relation_name: String::new(),
                    trace_context: None,
                    sync_token: None,
                    seq: 0,
                },
            },
            generation: 1,
        });

        // New structure event — data should NOT be cleared
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "list".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 2,
        });

        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        // Data persists: still 1 row, but render changed to "list"
        assert_eq!(debug_tag(&vm), "list:1");
    }

    #[tokio::test]
    async fn snapshot_returns_current_state() {
        let rq = ReactiveRenderedRows::new();
        rq.apply_event(UiEvent::Structure {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            candidates: vec![],
            generation: 1,
        });
        rq.apply_event(UiEvent::Data {
            batch: holon_api::streaming::BatchMapChangeWithMetadata {
                inner: holon_api::streaming::Batch {
                    items: vec![
                        holon_api::Change::Created {
                            data: make_row("r1", "a"),
                            origin: remote_origin(),
                        },
                        holon_api::Change::Created {
                            data: make_row("r2", "b"),
                            origin: remote_origin(),
                        },
                    ],
                },
                metadata: holon_api::streaming::BatchMetadata {
                    relation_name: String::new(),
                    trace_context: None,
                    sync_token: None,
                    seq: 0,
                },
            },
            generation: 1,
        });

        let (expr, rows) = rq.snapshot();
        assert!(matches!(expr, RenderExpr::FunctionCall { .. }));
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn registry_returns_same_instance() {
        let registry = ReactiveRegistry::new();
        let id = EntityUri::block("test-1");
        let a = registry.get_or_create(&id);
        let b = registry.get_or_create(&id);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn row_signal_vec_emits_per_row() {
        let rq = ReactiveRenderedRows::new();
        rq.set_generation(1);

        rq.apply_change(
            holon_api::Change::Created {
                data: enriched(make_row("a", "alpha")),
                origin: remote_origin(),
            },
            1,
        );
        rq.apply_change(
            holon_api::Change::Created {
                data: enriched(make_row("b", "beta")),
                origin: remote_origin(),
            },
            1,
        );

        // Verify rows are in the BTreeMap (row_signal_vec tested via
        // ReactiveCollection)
        let (_, rows) = rq.snapshot();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("id").unwrap().as_string().unwrap(), "a");
        assert_eq!(rows[1].get("id").unwrap().as_string().unwrap(), "b");
    }

    /// Regression / A-B proof (dogfood 2026-07-10, Martin ruling 2026-07-11):
    /// an id-less VALUE row (an aggregate / rule-trigger result — e.g. the
    /// journals `SELECT today AS name FROM clock` machinery) flowing through
    /// the reactive row set must NOT panic the render worker. Before the
    /// fix, `apply_change`'s `Created` arm did
    /// `data_row_entity_uri(&row).expect(...)` and killed the worker
    /// (blanking the page with a silent -32603). Now the row is keyed on
    /// its deterministic content hash and accumulated. Reverting
    /// the `RowIdentity`-based keying in `apply_change` makes THIS test panic —
    /// the same signal `inv-no-observed-errors` observes in the keystone.
    #[test]
    fn id_less_value_row_does_not_panic_apply_change() {
        let rq = ReactiveRenderedRows::new();
        rq.set_generation(1);

        // A value row: only `name`, NO `id` column (journals trigger shape).
        let mut value_row = DataRow::new();
        value_row.insert("name".to_string(), Value::String("2026-07-10".to_string()));

        rq.apply_change(
            holon_api::Change::Created {
                data: enriched(value_row.clone()),
                origin: remote_origin(),
            },
            1,
        );

        let (_, rows) = rq.snapshot();
        assert_eq!(rows.len(), 1, "value row must be accumulated, not dropped");
        assert_eq!(
            rows[0].get("name").unwrap().as_string().unwrap(),
            "2026-07-10"
        );

        // Recompute re-emits the same content → same content-hash key → still
        // one row (stable identity across incremental matview recompute).
        rq.apply_change(
            holon_api::Change::Created {
                data: enriched(value_row),
                origin: remote_origin(),
            },
            1,
        );
        let (_, rows) = rq.snapshot();
        assert_eq!(
            rows.len(),
            1,
            "identical value row on recompute must reuse identity, not duplicate"
        );
    }

    /// Snapshot path regression test: a root-layout-shaped tree with a
    /// streaming-style template (`columns(item_template: live_block())`) must
    /// still render its children when interpreted through the snapshot path
    /// (no live `data_source`).
    ///
    /// Before PR2, the macro's `(Some template, Some data_source)` arm
    /// discarded eagerly-interpreted items and created an empty streaming
    /// `ReactiveView`; the `(Some, None)` snapshot arm worked by accident.
    /// After PR2, both arms are structurally distinct via `CollectionData`,
    /// and the snapshot arm must still eagerly materialize items from
    /// `ctx.data_rows`.
    ///
    /// This test exercises the full path:
    ///   `interpret_pure → macro Collection → Static arm → static_collection
    ///    → ReactiveView.items populated`.
    #[test]
    fn snapshot_path_populates_collection_items() {
        use holon_api::render_types::Arg;
        use holon_api::render_types::RenderExpr;

        // columns(#{item_template: live_block()}) — same shape the root layout
        // uses. When data_source is None (snapshot path), the macro falls into
        // the `(Some tmpl, None ds)` arm and eagerly interprets.
        let expr = RenderExpr::FunctionCall {
            name: "columns".to_string(),
            args: vec![Arg {
                name: Some("item_template".to_string()),
                value: RenderExpr::FunctionCall {
                    name: "live_block".to_string(),
                    args: vec![],
                },
            }],
        };

        // Three fake region rows — roughly matches the shape the root layout
        // passes into columns: each has an id and a content.
        let rows: Vec<Arc<DataRow>> = ["left", "main", "right"]
            .iter()
            .map(|name| {
                let mut row = DataRow::new();
                row.insert("id".to_string(), Value::String(format!("block:{name}")));
                row.insert("content".to_string(), Value::String(name.to_string()));
                Arc::new(row)
            })
            .collect();

        let services = StubBuilderServices::new();
        let tree = interpret_pure(&expr, &rows, &services);

        // Expect a collection-backed node whose `items` MutableVec holds exactly
        // three LiveBlock children — one per row. If this asserts length zero,
        // the macro is routing snapshot-path calls through the Streaming arm
        // by mistake (which has no items until a driver runs).
        let view = tree
            .collection
            .as_ref()
            .unwrap_or_else(|| panic!("expected collection, got {:?}", tree.widget_name()));
        let items = view.items.lock_ref();
        assert_eq!(
            items.len(),
            3,
            "snapshot path should eagerly materialize 3 items from ctx.data_rows, got {}",
            items.len()
        );
        for (i, item) in items.iter().enumerate() {
            assert_eq!(
                item.widget_name().as_deref(),
                Some("live_block"),
                "item[{i}] should be live_block, got {:?}",
                item.widget_name()
            );
        }
    }

    // ── follow_dangling_link: create-response → navigation target ────────────

    #[test]
    fn dangling_link_nav_target_focuses_leaf_page_and_resets_main_scroll() {
        let response = Some(Value::String("block:leaf-123".to_string()));
        let (leaf, reset_scroll) =
            dangling_link_nav_target(&response, "main").expect("well-formed leaf id");
        assert_eq!(leaf.to_string(), "block:leaf-123");
        assert!(
            reset_scroll,
            "a main-region page open must reset the main-panel scroll"
        );
    }

    #[test]
    fn dangling_link_nav_target_non_main_region_leaves_scroll_alone() {
        let response = Some(Value::String("block:leaf-9".to_string()));
        let (_leaf, reset_scroll) =
            dangling_link_nav_target(&response, "right").expect("well-formed leaf id");
        assert!(
            !reset_scroll,
            "a non-main region (e.g. a right-sidebar pin) must not reset main scroll"
        );
    }

    #[test]
    fn dangling_link_nav_target_fails_loud_on_unexpected_response() {
        // The op contract is a `Value::String` leaf id; anything else (a None,
        // an Object, a number) is a fail-loud contract violation, not a
        // silently-dropped navigation.
        for bad in [
            None,
            Some(Value::Object(HashMap::new())),
            Some(Value::Integer(7)),
        ] {
            let err = dangling_link_nav_target(&bad, "main")
                .expect_err("unexpected response shape must fail loud");
            assert!(
                err.to_string().contains("create_page_from_link"),
                "error must name the offending op, got: {err}"
            );
        }
    }

    #[test]
    fn dangling_nav_applied_when_focus_unchanged_since_click() {
        // ALLOW(entity_uri_from_raw): test literal.
        let at_click = Some(EntityUri::from_raw("block:src"));
        let now = at_click.clone();
        assert!(
            !dangling_nav_superseded(&at_click, &now),
            "focus unchanged since click → navigation is still current, must apply"
        );
        // Also holds when nothing was focused at click and still isn't.
        assert!(!dangling_nav_superseded(&None, &None));
    }

    #[test]
    fn dangling_nav_skipped_when_focus_moved_during_create() {
        // ALLOW(entity_uri_from_raw): test literal.
        let at_click = Some(EntityUri::from_raw("block:src"));
        // ALLOW(entity_uri_from_raw): test literal.
        let now = Some(EntityUri::from_raw("block:elsewhere"));
        assert!(
            dangling_nav_superseded(&at_click, &now),
            "user navigated during the async create → stale task must NOT stomp newer focus"
        );
        // A clear→focus or focus→clear move is likewise a supersession.
        assert!(dangling_nav_superseded(&at_click, &None));
        assert!(dangling_nav_superseded(&None, &at_click));
    }
}
