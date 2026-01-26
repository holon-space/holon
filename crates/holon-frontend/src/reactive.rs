//! Reactive middle layer using futures-signals.
//!
//! Replaces CdcAccumulator + BlockWatchRegistry + AppState with a single
//! reactive cache. Each watched block or live query gets a `ReactiveQueryResults`
//! that IS the cache, the accumulator, AND the signal source.
//!
//! ```text
//! Turso IVM → UiEvent → ReactiveQueryResults → Signal<ViewModel> → Stream → Frontend
//!                        (IS the cache)         (IS the join)       (IS the API)
//! ```

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_signals::map_ref;
use futures_signals::signal::{Mutable, ReadOnlyMutable, Signal, SignalExt};
use futures_signals::signal_map::MutableBTreeMap;
use futures_signals::signal_vec::{SignalVec, SignalVecExt};

use holon_api::render_types::RenderExpr;
use holon_api::streaming::UiEvent;
use holon_api::widget_spec::{DataRow, EnrichedRow};
use holon_api::{ptr_identity, EntityUri, QueryLanguage, ReactiveRowProvider};

use crate::editable_text_provider::EditableTextProvider;
use crate::reactive_view_model::ReactiveViewModel;
use crate::render_context::RenderContext;
use crate::render_interpreter::RenderInterpreter;
use crate::view_model::ViewModel;
use crate::{FrontendSession, WidgetState};
use fluxdi::{Injector, Provider, Shared};

// ── BuilderServices trait ───────────────────────────────────────────────

/// Narrow capabilities available to builders during interpretation.
///
/// `ReactiveEngine` implements this. Builders never see `FrontendSession`
/// or `ReactiveEngine` directly — they call these methods through
/// `ctx.services` (an `Arc<dyn BuilderServices>`).
pub trait BuilderServices: Send + Sync {
    /// Interpret `expr` against `ctx` using this services' shadow interpreter.
    ///
    /// The implementation passes `self` as `&dyn BuilderServices` to the
    /// interpreter so recursive builder calls stay inside the same engine.
    /// This is the one and only entry point for row interpretation in the
    /// reactive pipeline — no caller ever touches `RenderInterpreter` directly.
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel;

    /// Get the current (RenderExpr, Vec<Arc<DataRow>>) for a block, ensuring a watcher is running.
    fn get_block_data(&self, id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>);

    /// Resolve the entity profile for a data row. Returns `None` when no entity type
    /// could be inferred.
    fn resolve_profile(&self, row: &DataRow) -> Option<holon::entity_profile::RowProfile>;

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
    fn profile_signal(&self) -> Mutable<Arc<holon::entity_profile::ProfileCache>> {
        Mutable::new(Arc::new(holon::entity_profile::ProfileCache::empty()))
    }

    /// Get the virtual child config for an entity type, if declared in its profile.
    fn virtual_child_config(
        &self,
        entity_name: &str,
    ) -> Option<holon::entity_profile::VirtualChildConfig> {
        None
    }

    /// Compile a query string to SQL.
    fn compile_to_sql(&self, query: &str, lang: QueryLanguage) -> Result<String>;

    /// Start a live query stream. Blocks until the stream is established.
    fn start_query(
        &self,
        sql: String,
        ctx: Option<crate::QueryContext>,
    ) -> Result<crate::RowChangeStream>;

    /// Look up widget state by block ID.
    fn widget_state(&self, id: &str) -> WidgetState;

    /// Fire-and-forget operation dispatch.
    ///
    /// Spawns the operation on the runtime and logs errors. This replaces the
    /// pattern of downcasting to `ReactiveEngine` just to get `session()` +
    /// `runtime_handle()` and calling `dispatch_operation()` manually.
    fn dispatch_intent(&self, intent: crate::operations::OperationIntent);

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
    fn ui_state(&self, _block_id: &EntityUri) -> HashMap<String, holon_api::Value> {
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
    /// Used by RenderContext::with_operations() to join keybindings into operations.
    fn key_bindings_snapshot(&self) -> std::collections::BTreeMap<String, holon_api::KeyChord> {
        std::collections::BTreeMap::new()
    }

    // ── UI state (focus, view mode) ─────────────────────────────────────

    /// Get the currently focused block ID.
    fn focused_block(&self) -> Option<EntityUri> {
        None
    }

    /// Cloned handle to the focused-block `Mutable`, when this services
    /// instance is backed by a `UiState`. Returns `None` for headless /
    /// stub services (no focus tracking). Used by reactive row
    /// providers like `focus_chain` that need a long-lived signal
    /// source rather than a one-shot snapshot.
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
    fn set_focus(&self, _block_id: Option<EntityUri>) {}

    /// Get a `MutableText` handle for collaborative editing of a block field.
    ///
    /// Returns `Err` for headless/stub services that don't have a LoroDoc.
    fn editable_text(
        &self,
        _block_id: &str,
        _field: &str,
    ) -> anyhow::Result<holon::sync::mutable_text::MutableText> {
        Err(anyhow::anyhow!(
            "editable_text not supported by this BuilderServices implementation"
        ))
    }

    /// Fully-resolved static snapshot of a block's UI tree.
    ///
    /// Interprets the block's render expression against its current data rows,
    /// then recursively resolves every nested `LiveBlock` placeholder by calling
    /// itself for each embedded block. Returns a `ViewModel` suitable for
    /// serialization (MCP `describe_ui`, PBT assertions, TUI rendering).
    ///
    /// Default implementation composes `get_block_data` + `interpret_with_source`
    /// + `snapshot_resolved`. Implementors with an optimized watcher path (e.g.
    /// `ReactiveEngine::ensure_watching`) can override.
    fn snapshot_resolved(&self, block_id: &EntityUri) -> crate::view_model::ViewModel {
        let (expr, rows) = self.get_block_data(block_id);
        let ctx = RenderContext {
            data_rows: rows,
            available_space: self.viewport_snapshot(),
            ..Default::default()
        };
        let rvm = self.interpret(&expr, &ctx);
        rvm.snapshot_resolved(&|bid| self.snapshot_resolved(bid))
    }

    /// Wait until the first Structure event has been received for a block.
    /// Returns immediately if the block's render expression is already loaded.
    /// Default: returns a ready future (for headless/stub impls that don't stream).
    fn await_ready(
        &self,
        _id: &EntityUri,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }

    /// Get a reactive signal for a block's UI. Each call returns an independent
    /// signal that tracks the block's render expression + data.
    fn watch_block_signal(
        &self,
        _block_id: &EntityUri,
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
    fn watch_live(
        &self,
        _block_id: &EntityUri,
        _services: Arc<dyn BuilderServices>,
    ) -> crate::LiveBlock {
        panic!("watch_live not supported by this BuilderServices implementation")
    }

    /// Stop watching a block and release its reactive state (watchers, MutableVec items).
    /// No-op by default. Implemented by ReactiveEngine.
    fn unwatch(&self, _block_id: &EntityUri) {}

    /// Get a reactive signal for a live query's UI. GPUI polls this directly.
    fn watch_query_signal(
        &self,
        _sql: String,
        _render_expr: holon_api::render_types::RenderExpr,
        _query_context: Option<crate::QueryContext>,
    ) -> std::pin::Pin<
        Box<dyn futures_signals::signal::Signal<Item = crate::ReactiveViewModel> + Send>,
    > {
        panic!("watch_query_signal not supported by this BuilderServices implementation")
    }

    /// Reactive signal that fires when the focused editor's cursor moves
    /// (driven by the `current_editor_focus` matview). Each `EditorView`
    /// subscribes and acts only on emissions whose `block_id` matches its
    /// own `row_id`. Returns `None` for sync/test services that don't run
    /// a CDC watcher.
    fn watch_editor_cursor(
        &self,
    ) -> Option<
        std::pin::Pin<
            Box<dyn futures_signals::signal::Signal<Item = Option<(String, i64)>> + Send>,
        >,
    > {
        None
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

    /// Execute a SQL query for popup/autocomplete providers (doc-link,
    /// command palette). Minimal async capability that replaces the
    /// `Arc<FrontendSession>` plumb line through the editor. Headless/stub
    /// impls without a backend return `Err`.
    fn popup_query(
        &self,
        sql: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>;
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
/// Used by `ReactiveQueryResults` (which adds a RenderExpr for interpretation)
/// and directly by raw CDC watchers like `watch_editor_cursor`.
pub struct ReactiveRowSet {
    data: MutableBTreeMap<String, Mutable<Arc<DataRow>>>,
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
    pub fn set_generation(&self, gen: u64) {
        self.generation.set(gen);
    }

    /// Current generation.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Apply a single enriched row-level CDC change. Ignores stale generations.
    ///
    /// Accepts `Change<EnrichedRow>` — the caller must have gone through
    /// `enrich_row()` / `enrich_stream()` to obtain enriched data.
    /// This prevents accidentally feeding raw storage data into the reactive pipeline.
    pub fn apply_change(&self, change: holon_api::Change<EnrichedRow>, generation: u64) {
        if generation != self.generation.get() {
            return;
        }
        match change {
            holon_api::Change::Created { data, .. } => {
                let row = Arc::new(data.into_inner());
                let key = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("Created event must have 'id' column")
                    .to_string();
                let mut lock = self.data.lock_mut();
                if let Some(existing) = lock.get(&key) {
                    // Defensive: a Created arriving for a row we already know
                    // about — treat as Updated to avoid losing the cell identity.
                    existing.set(row);
                } else {
                    lock.insert_cloned(key, Mutable::new(row));
                }
            }
            holon_api::Change::Updated { id, data, .. } => {
                let row = Arc::new(data.into_inner());
                let mut lock = self.data.lock_mut();
                if let Some(existing) = lock.get(&id) {
                    existing.set(row);
                } else {
                    // Out-of-order: Updated before Created. Insert so we don't
                    // drop the row.
                    lock.insert_cloned(id, Mutable::new(row));
                }
            }
            holon_api::Change::Deleted { id, .. } => {
                self.data.lock_mut().remove(&id);
            }
            holon_api::Change::FieldsChanged {
                entity_id, fields, ..
            } => {
                let lock = self.data.lock_ref();
                if let Some(existing) = lock.get(&entity_id) {
                    let mut patched = existing.get_cloned();
                    let map = Arc::make_mut(&mut patched);
                    for (name, _old, new) in fields {
                        map.insert(name, new);
                    }
                    existing.set(patched);
                }
            }
        }
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
    pub fn row_mutable(&self, id: &str) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
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
    pub fn data_signal(&self) -> impl Signal<Item = Vec<(String, Arc<DataRow>)>> {
        self.data
            .entries_cloned()
            .map_signal(|(k, cell)| cell.signal_cloned().map(move |v| (k.clone(), v)))
            .to_signal_cloned()
    }

    /// Per-row `SignalVec` with keys. Each item is `(entity_id, Arc<DataRow>)`.
    ///
    /// Used by `MutableTree` to translate keyed VecDiff into tree operations.
    /// Unlike `row_signal_vec()`, preserves the entity ID for `RemoveAt` tracking.
    pub fn keyed_signal_vec(&self) -> impl SignalVec<Item = (String, Arc<DataRow>)> {
        self.data
            .entries_cloned()
            .map_signal(|(k, cell)| cell.signal_cloned().map(move |v| (k.clone(), v)))
    }
}

// ── ReactiveRowProvider impls ────────────────────────────────────────────
//
// Exposes `ReactiveRowSet` and `ReactiveQueryResults` through the trait
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
    ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
        Box::pin(self.keyed_signal_vec())
    }
    fn cache_identity(&self) -> u64 {
        ptr_identity(self)
    }
    fn row_mutable(&self, id: &str) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
        self.row_mutable(id)
    }
}

// ── ReactiveQueryResults ─────────────────────────────────────────────────

/// Reactive state for one query's result set + how to render it.
///
/// Composes a `ReactiveRowSet` (data accumulation) with a `RenderExpr`
/// (how to visualize it). Signals combine both into `Signal<ReactiveViewModel>`.
pub struct ReactiveQueryResults {
    render_expr: Mutable<RenderExpr>,
    rows: ReactiveRowSet,
    structure_ready: tokio::sync::Notify,
}

impl ReactiveQueryResults {
    pub fn new() -> Self {
        Self {
            render_expr: Mutable::new(loading_expr()),
            rows: ReactiveRowSet::new(),
            structure_ready: tokio::sync::Notify::new(),
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
            self.render_expr.get_cloned(),
            RenderExpr::FunctionCall { ref name, .. } if name == "loading"
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
    pub fn set_generation(&self, gen: u64) {
        self.rows.set_generation(gen);
    }

    /// Apply a UiEvent directly. Single entry point for all CDC events.
    ///
    /// Structure → sets render_expr + generation. Does NOT clear data — the new
    /// data stream will overwrite when it arrives, avoiding flash of empty content.
    /// Data → row-level diffs into the row set. Stale generations discarded.
    pub fn apply_event(&self, event: UiEvent) {
        match event {
            UiEvent::Structure {
                render_expr,
                candidates: _,
                generation,
            } => {
                self.rows.set_generation(generation);
                if self.render_expr.get_cloned() != render_expr {
                    self.render_expr.set(render_expr);
                }
                self.structure_ready.notify_waiters();
            }
            UiEvent::Data { batch, generation } => {
                if generation != self.rows.generation() {
                    return;
                }
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
                    self.rows.apply_change(enriched, generation);
                }
            }
        }
    }

    /// Apply a single enriched row-level CDC change. Delegates to the inner row set.
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
    pub fn data_signal(&self) -> impl Signal<Item = Vec<(String, Arc<DataRow>)>> {
        self.rows.data_signal()
    }

    /// Per-row keyed `SignalVec`. Delegates to the inner row set.
    pub fn keyed_signal_vec(&self) -> impl SignalVec<Item = (String, Arc<DataRow>)> {
        self.rows.keyed_signal_vec()
    }

    /// Signal that emits a new `ReactiveViewModel` whenever render_expr or data changes.
    ///
    /// `interpret_fn` transforms `(&RenderExpr, &[Arc<DataRow>]) → ReactiveViewModel`.
    ///
    /// **Note**: This re-interprets the ENTIRE tree on every change (structural OR data).
    /// For per-row collection updates, use `structural_signal()` + `ReactiveCollection`.
    pub fn reactive_signal<F: ?Sized>(
        &self,
        interpret_fn: Arc<F>,
    ) -> impl Signal<Item = ReactiveViewModel>
    where
        F: Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
    {
        self.reactive_signal_with_ui_gen(interpret_fn, futures_signals::signal::always(0u64))
    }

    /// Like `reactive_signal` but also re-interprets when `ui_gen_signal` fires.
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

    /// Like `structural_signal` but also re-interprets when `ui_gen_signal` fires.
    ///
    /// Fires on render_expr change OR ui_state change (focus, view mode).
    /// Data-only changes do NOT trigger — those are handled by ReactiveView drivers.
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

impl ReactiveRowProvider for ReactiveQueryResults {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        self.rows.snapshot_rows()
    }
    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        Box::pin(self.rows.row_signal_vec())
    }
    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
        Box::pin(self.rows.keyed_signal_vec())
    }
    fn row_mutable(&self, id: &str) -> Option<ReadOnlyMutable<Arc<DataRow>>> {
        self.rows.row_mutable(id)
    }
    fn cache_identity(&self) -> u64 {
        // Inner row-set pointer — two `ReactiveQueryResults` wrapping
        // the same rows would share identity, which is what the cache
        // wants.
        ptr_identity(&self.rows)
    }
}

// ── ReactiveRegistry ─────────────────────────────────────────────────────

/// Internal registry of ReactiveQueryResults, keyed by EntityUri.
///
/// Thread-safe: the HashMap is behind a Mutex, but individual
/// ReactiveQueryResults fields use futures-signals' lock-free primitives.
struct ReactiveRegistry {
    entries: Mutex<HashMap<EntityUri, Arc<ReactiveQueryResults>>>,
}

impl ReactiveRegistry {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_create(&self, id: &EntityUri) -> Arc<ReactiveQueryResults> {
        self.entries
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert_with(|| Arc::new(ReactiveQueryResults::new()))
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
    /// Number of active consumers (ReactiveShell instances) watching this block.
    /// When refcount drops to 0, the watcher is eligible for cleanup.
    refcount: usize,
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
    /// Currently focused block (receives `is_focused = true` in predicate context).
    focused_block: Mutable<Option<EntityUri>>,
    /// Monotonically increasing counter, bumped when the viewport changes.
    /// Included in `ReactiveQueryResults::reactive_signal` so that viewport
    /// changes trigger re-interpretation of affected blocks (breakpoint updates).
    /// View mode and expand state are now handled by node-owned `Mutable`s
    /// (no engine-level caches).
    viewport_generation: Mutable<u64>,
    /// Root viewport allocation. The platform shell pushes updates here on
    /// resize / keyboard / rotation events; the root `ReactiveView`'s
    /// `space` Mutable mirrors this, which starts the container-query
    /// cascade down through partitioning layout containers.
    viewport: Mutable<Option<ViewportInfo>>,
}

impl UiState {
    fn new() -> Self {
        Self {
            focused_block: Mutable::new(None),
            viewport_generation: Mutable::new(0),
            viewport: Mutable::new(None),
        }
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
    /// `ui_generation` would cause LiveQueryView to replace its entire tree
    /// (re-creating editors for all 269 rows), producing multiple cursors.
    pub fn set_focus(&self, block_id: Option<EntityUri>) {
        if self.focused_block.get_cloned() == block_id {
            return;
        }
        self.focused_block.set(block_id);
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
    /// Bumps `viewport_generation` because breakpoint changes alter the selected
    /// variant and therefore the render expression (structural change).
    /// `Mutable::set_neq` dedups equal values — no-op updates never fire the signal graph.
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
    /// Note: `view_mode` and `is_expanded` are added by `ReactiveEngine::ui_state()`
    /// from the engine's keyed caches, not here.
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

        // Global viewport fallback: emitted so blocks not reached by any
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

/// The reactive middle layer. Replaces BlockWatchRegistry + AppState.
///
/// Manages per-block `ReactiveQueryResults` instances. Each block's UiEvent
/// stream feeds its ReactiveQueryResults; the signal graph produces ViewModels
/// on demand. Frontends consume via `watch()` (stream) or `snapshot()` (polling).
pub struct ReactiveEngine {
    registry: ReactiveRegistry,
    session: Arc<FrontendSession>,
    pub runtime_handle: tokio::runtime::Handle,
    interpret_fn: Arc<dyn Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync>,
    /// The shared shadow interpreter, built once by `HolonFrontendModule::configure()`
    /// and injected here via DI. Used by `BuilderServices::interpret`.
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    watchers: Mutex<HashMap<EntityUri, WatcherState>>,
    ui_state: UiState,
    /// Reactive keybinding registry: operation_name → key chord.
    /// Keybindings are joined into OperationDescriptors during ViewModel construction.
    key_bindings: MutableBTreeMap<String, holon_api::KeyChord>,
    /// Shared Weak-ref cache of `ReactiveRowProvider`s produced by
    /// value functions like `focus_chain()` / `ops_of(uri)`. Reused
    /// across render passes so identical `(name, args)` calls share an
    /// Arc instead of each building a fresh provider.
    provider_cache: Arc<crate::provider_cache::ProviderCache>,
    /// Shared editor-cursor signal source. Lazily spawned on the first
    /// `watch_editor_cursor()` call so we have at most one CDC stream on
    /// `current_editor_focus` regardless of how many editors subscribe.
    editor_cursor: Mutex<Option<Mutable<Option<(String, i64)>>>>,
    /// Optional MutableText provider. When `Some`, `BuilderServices::editable_text()`
    /// delegates to it. Set by test harnesses that want CRDT-backed editors.
    pub editable_text_provider:
        Mutex<Option<Arc<crate::editable_text_provider::LoroEditableTextProvider>>>,
}

impl ReactiveEngine {
    pub fn new(
        session: Arc<FrontendSession>,
        runtime_handle: tokio::runtime::Handle,
        interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
        interpret_fn: impl Fn(&RenderExpr, &[Arc<DataRow>]) -> ReactiveViewModel + Send + Sync + 'static,
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
            editor_cursor: Mutex::new(None),
            editable_text_provider: Mutex::new(None),
        }
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
    /// The signal re-evaluates when the block's render expression or data changes.
    /// Poll this directly from a GPUI `cx.spawn` — no intermediate channel needed.
    /// CDC writes from tokio wake the signal cross-thread.
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
    /// Convenience wrapper over `watch_signal()` for consumers that need a Stream
    /// (non-GPUI frontends, tests). Prefer `watch_signal()` + `for_each` for GPUI.
    pub fn watch(
        &self,
        block_id: &EntityUri,
    ) -> Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>> {
        Box::pin(self.watch_signal(block_id).to_stream())
    }

    /// Watch a block with per-row collection reactivity.
    ///
    /// Returns a `LiveBlock` whose `tree` contains `ReactiveChildren` with
    /// `MutableVec`s that are updated per-row in the background. The
    /// `structural_changes` stream emits only when the render expression
    /// changes — data-only changes update the tree in-place via the MutableVec.
    ///
    /// `services` must be `Arc<ReactiveEngine>` cast to `Arc<dyn BuilderServices>`.
    /// (Passed explicitly to avoid self-referential `Arc<Self>` inside the engine.)
    pub fn watch_live(
        &self,
        block_id: &EntityUri,
        services: Arc<dyn BuilderServices>,
    ) -> LiveBlock {
        let results = self.ensure_watching(block_id);

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
            data_rows: rows,
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
                    data_rows: rows.to_vec(),
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
            tracing::warn!(
                block_id = %block_id,
                "snapshot: cycle detected in LiveBlock resolution; returning empty ViewModel"
            );
            return crate::ViewModel::error(
                "error",
                format!("cycle in LiveBlock resolution for {block_id}"),
            );
        }

        // Ensure we always remove from visited on exit (even on panic).
        struct Guard(EntityUri);
        impl Drop for Guard {
            fn drop(&mut self) {
                VISITED.with(|v| {
                    v.borrow_mut().remove(&self.0);
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

    /// Get the ReactiveQueryResults for a block, ensuring a watcher is running.
    /// Used by the interpreter's live_block builder and by tests.
    pub fn ensure_watching(&self, block_id: &EntityUri) -> Arc<ReactiveQueryResults> {
        let results = self.registry.get_or_create(block_id);

        let mut watchers = self.watchers.lock().unwrap();
        if let Some(state) = watchers.get_mut(block_id) {
            state.refcount += 1;
            return results;
        }

        let session = self.session.clone();
        let reactive = results.clone();
        let bid = block_id.clone();

        let (proxy_cmd_tx, mut proxy_cmd_rx) =
            tokio::sync::mpsc::channel::<holon_api::WatcherCommand>(16);

        let task = self.runtime_handle.spawn(async move {
            match session.watch_ui(&bid).await {
                Ok(watch) => {
                    let (mut event_rx, cmd_tx) = watch.into_parts();

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
                                        "[mp_event] Data gen={generation} (current={cur_gen}) items={n}{}",
                                        if dropped { " DROPPED-stale-gen" } else { "" }
                                    );
                                    // Per-change detail. Lets the next debugging session
                                    // confirm whether the matview CDC layer surfaces an
                                    // `Updated` for the modified block after split_block /
                                    // set_field — see HANDOFF_TUI_RENDER.md "third pass".
                                    for (i, change) in batch.inner.items.iter().enumerate() {
                                        let snippet = |row: &holon_api::widget_spec::DataRow| -> String {
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
                                                    "[mp_event]   change[{i}]: Created id={id} content={:?}",
                                                    snippet(data)
                                                );
                                            }
                                            holon_api::Change::Updated { id, data, .. } => {
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: Updated id={id} content={:?}",
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
                                                let names: Vec<&str> =
                                                    fields.iter().map(|(n, _, _)| n.as_str()).collect();
                                                tracing::trace!(
                                                    "[mp_event]   change[{i}]: FieldsChanged id={entity_id} fields={:?}",
                                                    names
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        reactive.apply_event(event);
                        if bid.as_str() == "block:default-main-panel" {
                            let rows_n = reactive.rows.snapshot_rows().len();
                            tracing::trace!("[mp_event] post-apply rows.len={rows_n}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("watch_ui({bid}) failed: {e}");
                }
            }
        });

        watchers.insert(
            block_id.clone(),
            WatcherState {
                task,
                command_tx: proxy_cmd_tx,
                refcount: 1,
            },
        );

        results
    }

    /// Watch a live query as a `Signal`. GPUI polls this directly.
    pub fn watch_query_signal(
        &self,
        sql: String,
        render_expr: RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> Pin<Box<dyn Signal<Item = ReactiveViewModel> + Send>> {
        let results = self.ensure_query_watching(sql, render_expr, query_context);
        results
            .reactive_signal_with_ui_gen(
                self.interpret_fn.clone(),
                self.ui_state.generation_signal(),
            )
            .boxed()
    }

    /// Watch a live query as a `Stream`. For non-GPUI consumers.
    pub fn watch_query(
        &self,
        sql: String,
        render_expr: RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>> {
        Box::pin(
            self.watch_query_signal(sql, render_expr, query_context)
                .to_stream(),
        )
    }

    /// Ensure a query watcher is running and return its ReactiveQueryResults.
    fn ensure_query_watching(
        &self,
        sql: String,
        render_expr: RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> Arc<ReactiveQueryResults> {
        let key = EntityUri::from_raw(&format!("query:{}", hash_query(&sql)));
        let results = self.registry.get_or_create(&key);
        results.set_render_expr(render_expr);
        results.set_generation(1);

        let mut watchers = self.watchers.lock().unwrap();
        if !watchers.contains_key(&key) {
            let session = self.session.clone();
            let reactive = results.clone();
            let task = self.runtime_handle.spawn(async move {
                match session
                    .query_and_watch(sql, HashMap::new(), query_context)
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
                        tracing::warn!("query_and_watch failed: {e}");
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

        results
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

    /// Watch the editor cursor state via CDC on `current_editor_focus`.
    ///
    /// Returns a Signal that fires `Some((block_id, cursor_offset))` when
    /// an `editor_focus` operation updates the cursor (e.g., after split_block).
    ///
    /// Uses the raw CDC stream directly (no enrichment) because editor_cursor
    /// rows have no `id` column — they're keyed by (region, block_id).
    pub fn watch_editor_cursor(&self) -> Pin<Box<dyn Signal<Item = Option<(String, i64)>> + Send>> {
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        use holon_api::streaming::Change;

        // Reuse the shared Mutable across all subscribers so we keep at
        // most one CDC stream on `current_editor_focus` regardless of how
        // many editors subscribe (one per visible row).
        let mut guard = self.editor_cursor.lock().unwrap();
        if let Some(ref cursor) = *guard {
            return cursor.signal_cloned().boxed();
        }
        let cursor: Mutable<Option<(String, i64)>> = Mutable::new(None);
        let writer = cursor.clone();

        let engine = self.session.engine().clone();
        self.runtime_handle.spawn(async move {
            let sql =
                "SELECT block_id, cursor_offset, updated_at FROM current_editor_focus WHERE region = 'main'"
                    .to_string();
            // Track the latest timestamp to ignore stale CDC re-emissions.
            // When any editor_cursor row changes, the matview CDC may re-emit
            // multiple rows — we only want the most recently updated one.
            let mut latest_ts = String::new();
            match engine.query_and_watch(sql, HashMap::new(), None).await {
                Ok(stream) => {
                    tokio::pin!(stream);
                    while let Some(batch) = stream.next().await {
                        let mut best: Option<(String, i64, String)> = None;
                        for row_change in batch.inner.items {
                            let data = match row_change.change {
                                Change::Created { data, .. } | Change::Updated { data, .. } => data,
                                _ => continue,
                            };
                            let ts = data
                                .get("updated_at")
                                .and_then(|v| v.as_string())
                                .unwrap_or("")
                                .to_string();
                            if ts < latest_ts {
                                continue;
                            }
                            let block_id = data
                                .get("block_id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string());
                            let offset = data
                                .get("cursor_offset")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            if let Some(block_id) = block_id {
                                if best.as_ref().map_or(true, |(_, _, t)| ts >= *t) {
                                    best = Some((block_id, offset, ts));
                                }
                            }
                        }
                        if let Some((block_id, offset, ts)) = best {
                            latest_ts = ts;
                            let new_val = Some((block_id, offset));
                            writer.set_if(new_val, |old, new| old != new);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("watch_editor_cursor: CDC setup failed: {e}");
                }
            }
        });

        let signal = cursor.signal_cloned().boxed();
        *guard = Some(cursor);
        signal
    }

    /// Decrement the refcount for a block's watcher. When the last consumer
    /// drops, the watcher task is aborted and reactive state is released.
    pub fn unwatch(&self, block_id: &EntityUri) {
        let mut watchers = self.watchers.lock().unwrap();
        let should_remove = match watchers.get_mut(block_id) {
            Some(state) => {
                state.refcount = state.refcount.saturating_sub(1);
                state.refcount == 0
            }
            None => false,
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

/// Default render expression for blocks whose watcher hasn't delivered data yet.
pub fn loading_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "loading".to_string(),
        args: vec![],
    }
}

fn table_expr() -> RenderExpr {
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
    /// Access the FrontendSession (for operation dispatch in frontend-specific builders).
    pub fn session(&self) -> &Arc<FrontendSession> {
        &self.session
    }

    /// Access the tokio runtime handle.
    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.runtime_handle
    }
}

impl BuilderServices for ReactiveEngine {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
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
    fn resolve_profile(&self, row: &DataRow) -> Option<holon::entity_profile::RowProfile> {
        self.session.resolve_row_profile(row)
    }

    fn profile_signal(&self) -> Mutable<Arc<holon::entity_profile::ProfileCache>> {
        self.session.engine().profile_resolver().profile_signal()
    }

    fn virtual_child_config(
        &self,
        entity_name: &str,
    ) -> Option<holon::entity_profile::VirtualChildConfig> {
        self.session
            .engine()
            .profile_resolver()
            .virtual_child_config(entity_name)
    }

    fn compile_to_sql(&self, query: &str, lang: QueryLanguage) -> Result<String> {
        self.session.engine().compile_to_sql(query, lang)
    }

    fn start_query(
        &self,
        sql: String,
        ctx: Option<crate::QueryContext>,
    ) -> Result<crate::RowChangeStream> {
        // TODO: Change BuilderServices::start_query return type to EnrichedChangeStream.
        // For now, use the raw BackendEngine path (bypasses FrontendSession enrichment).
        // This is safe because start_query's output feeds into ReactiveEngine's
        // ensure_query_watching which now also goes through the enriched path.
        let session = self.session.clone();
        let rt = self.runtime_handle.clone();
        std::thread::scope(|s| {
            s.spawn(|| rt.block_on(session.engine().query_and_watch(sql, HashMap::new(), ctx)))
                .join()
                .unwrap()
        })
    }

    fn widget_state(&self, id: &str) -> WidgetState {
        self.session.widget_state(id)
    }

    fn dispatch_intent(&self, intent: crate::operations::OperationIntent) {
        if intent.entity_name == "preferences" && intent.op_name == "set" {
            if let (Some(key), Some(value)) = (
                intent.params.get("key").and_then(|v| v.as_string()),
                intent.params.get("value"),
            ) {
                let pref_key = crate::preferences::PrefKey::new(&key);
                let toml_value = crate::preferences::value_to_toml(value);
                self.session.set_preference(&pref_key, toml_value);
            }
            return;
        }

        // Mirror `navigation.focus` into `UiState.focused_block` so
        // value-fn row providers (`focus_chain()`) see focus changes
        // without having to re-derive them from `navigation_cursor`.
        // The backend still writes the SQL tables; this just keeps the
        // frontend-side signal graph in sync.
        maybe_mirror_navigation_focus(&self.ui_state, &intent);

        crate::operations::dispatch_operation(
            &self.runtime_handle,
            &self.session,
            &intent.entity_name,
            intent.op_name,
            intent.params,
        );
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
            "present_op({}.{}): multi-param popup activation is not yet wired for op_button sites; \
             {} param(s) missing (follow-up to mobile-bar PR)",
            op.entity_name,
            op.name,
            matched.missing_params.len()
        );
    }

    fn dispatch_intent_sync(
        &self,
        intent: crate::operations::OperationIntent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        // Preference sets run inline — no async execute_operation path.
        if intent.entity_name == "preferences" && intent.op_name == "set" {
            if let (Some(key), Some(value)) = (
                intent.params.get("key").and_then(|v| v.as_string()),
                intent.params.get("value"),
            ) {
                let pref_key = crate::preferences::PrefKey::new(&key);
                let toml_value = crate::preferences::value_to_toml(value);
                self.session.set_preference(&pref_key, toml_value);
            }
            return Box::pin(std::future::ready(Ok(())));
        }

        maybe_mirror_navigation_focus(&self.ui_state, &intent);

        let session = self.session.clone();
        Box::pin(async move {
            session
                .execute_operation(&intent.entity_name, &intent.op_name, intent.params)
                .await
                .with_context(|| {
                    format!(
                        "dispatch_intent_sync: {}.{} failed",
                        intent.entity_name, intent.op_name
                    )
                })?;
            Ok(())
        })
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

    fn provider_cache(&self) -> Option<Arc<crate::provider_cache::ProviderCache>> {
        Some(self.provider_cache.clone())
    }

    fn set_focus(&self, block_id: Option<EntityUri>) {
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

    fn watch_query_signal(
        &self,
        sql: String,
        render_expr: holon_api::render_types::RenderExpr,
        query_context: Option<crate::QueryContext>,
    ) -> std::pin::Pin<
        Box<dyn futures_signals::signal::Signal<Item = crate::ReactiveViewModel> + Send>,
    > {
        ReactiveEngine::watch_query_signal(self, sql, render_expr, query_context)
    }

    fn watch_editor_cursor(
        &self,
    ) -> Option<
        std::pin::Pin<
            Box<dyn futures_signals::signal::Signal<Item = Option<(String, i64)>> + Send>,
        >,
    > {
        Some(ReactiveEngine::watch_editor_cursor(self))
    }

    fn unwatch(&self, block_id: &EntityUri) {
        ReactiveEngine::unwatch(self, block_id);
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime_handle.clone()
    }

    fn popup_query(
        &self,
        sql: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>
    {
        let session = self.session.clone();
        Box::pin(async move { session.execute_query(sql, HashMap::new(), None).await })
    }

    fn editable_text(
        &self,
        block_id: &str,
        field: &str,
    ) -> anyhow::Result<holon::sync::mutable_text::MutableText> {
        let guard = self.editable_text_provider.lock().unwrap();
        match &*guard {
            Some(p) => p.editable_text(block_id, field),
            None => Err(anyhow::anyhow!(
                "editable_text not configured for this ReactiveEngine"
            )),
        }
    }
}

/// Headless `BuilderServices` stub for MCP/tests that don't have a ReactiveEngine.
pub struct HeadlessBuilderServices {
    engine: Arc<holon::api::BackendEngine>,
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    rt_handle: tokio::runtime::Handle,
}

impl HeadlessBuilderServices {
    /// Construct from the current tokio runtime context. Panics loudly if
    /// called outside a tokio runtime — all real call sites already are.
    pub fn new(engine: Arc<holon::api::BackendEngine>) -> Self {
        Self::with_handle(engine, tokio::runtime::Handle::current())
    }

    pub fn with_handle(
        engine: Arc<holon::api::BackendEngine>,
        rt_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            engine,
            interpreter: Arc::new(crate::shadow_builders::build_shadow_interpreter()),
            rt_handle,
        }
    }
}

impl BuilderServices for HeadlessBuilderServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn get_block_data(&self, _id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (table_expr(), vec![])
    }

    fn resolve_profile(&self, row: &DataRow) -> Option<holon::entity_profile::RowProfile> {
        let (profile, _computed) = self.engine.profile_resolver().resolve_with_variants(row);
        Some(profile.as_ref().clone())
    }

    fn profile_signal(&self) -> Mutable<Arc<holon::entity_profile::ProfileCache>> {
        self.engine.profile_resolver().profile_signal()
    }

    fn compile_to_sql(&self, query: &str, lang: QueryLanguage) -> Result<String> {
        self.engine.compile_to_sql(query, lang)
    }

    fn start_query(
        &self,
        _sql: String,
        _ctx: Option<crate::QueryContext>,
    ) -> Result<crate::RowChangeStream> {
        anyhow::bail!("HeadlessBuilderServices does not support live queries")
    }

    fn widget_state(&self, _id: &str) -> WidgetState {
        WidgetState::default()
    }

    fn dispatch_intent(&self, intent: crate::operations::OperationIntent) {
        tracing::warn!(
            "HeadlessBuilderServices.dispatch_intent({}.{}) — no-op in headless mode",
            intent.entity_name,
            intent.op_name
        );
    }

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _ctx_params: HashMap<String, holon_api::Value>,
    ) {
        panic!(
            "HeadlessBuilderServices::present_op({}.{}) — op_button must not be \
             reached under a non-interactive services instance. Its YAML branch \
             is gated by `if_space(<600)` in an interactive session; reaching \
             this panic means the render path wired an interactive builder into \
             a headless one — fix the render path, do not swallow this.",
            op.entity_name, op.name
        );
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }

    fn popup_query(
        &self,
        sql: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>
    {
        let engine = self.engine.clone();
        Box::pin(async move { engine.execute_query(sql, HashMap::new(), None).await })
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

    fn get_block_data(&self, _id: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (table_expr(), vec![])
    }

    fn resolve_profile(&self, _row: &DataRow) -> Option<holon::entity_profile::RowProfile> {
        None
    }

    fn compile_to_sql(&self, _query: &str, _lang: QueryLanguage) -> Result<String> {
        anyhow::bail!("StubBuilderServices does not support query compilation")
    }

    fn start_query(
        &self,
        _sql: String,
        _ctx: Option<crate::QueryContext>,
    ) -> Result<crate::RowChangeStream> {
        anyhow::bail!("StubBuilderServices does not support live queries")
    }

    fn widget_state(&self, _id: &str) -> WidgetState {
        WidgetState::default()
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
        _ctx_params: HashMap<String, holon_api::Value>,
    ) {
        panic!(
            "StubBuilderServices::present_op({}.{}) — op_button must not be \
             reached under a stub services instance. If a gallery/example \
             renders the mobile action bar it should swap in a real \
             ReactiveEngine, not route through the stub.",
            op.entity_name, op.name
        );
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }

    fn popup_query(
        &self,
        _sql: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataRow>>> + Send + 'static>>
    {
        Box::pin(async { anyhow::bail!("StubBuilderServices does not support popup_query") })
    }
}

fn hash_query(sql: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sql.hash(&mut hasher);
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
/// Registered via `set_render_interpreter()`. Resolved by the ReactiveEngine factory.
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
    match intent.op_name.as_str() {
        // `focus` and `editor_focus` both move focus; the latter is what
        // a click dispatches (the GPUI click handler in
        // `frontends/gpui/src/render/builders/render_entity.rs:47-62`
        // calls `services.set_focus(Some(id))` *and* dispatches
        // `editor_focus`). Headless drivers (TUI, Flutter) skip the
        // direct `set_focus` call and rely solely on the dispatch,
        // so mirroring `editor_focus` here keeps `focused_block` in
        // sync across all frontends.
        "focus" | "editor_focus" => {
            let block_id = intent
                .params
                .get("block_id")
                .and_then(|v| v.as_string())
                .map(|s| EntityUri::from_raw(s));
            ui_state.set_focus(block_id);
        }
        "go_home" => ui_state.set_focus(None),
        // `go_back` / `go_forward` would require reading
        // `navigation_history` to know the target — leave them alone
        // until the backend grows a synchronous "current focus" accessor.
        _ => {}
    }
}

/// Pure ViewModel construction: render expression + data rows + services → ViewModel tree.
///
/// Thin free-function wrapper that forwards to `services.interpret_with_source`.
/// Retained so external callers (PBT reference model, widget gallery, tests) can
/// keep their existing call-site shape.
#[tracing::instrument(level = "debug", skip_all)]
pub fn interpret_pure(
    expr: &RenderExpr,
    rows: &[Arc<DataRow>],
    services: &dyn BuilderServices,
) -> ReactiveViewModel {
    let ctx = RenderContext {
        data_rows: rows.to_vec(),
        available_space: services.viewport_snapshot(),
        ..Default::default()
    };
    services.interpret(expr, &ctx)
}

/// Build the default interpret function for the ReactiveEngine.
///
/// Uses a `OnceLock<Arc<dyn BuilderServices>>` to break the circular dependency:
/// engine needs interpret_fn, interpret_fn needs services, services IS the engine.
/// The services are set after engine construction.
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
/// render expression changes — requiring a full rebuild (get a new `LiveBlock`).
pub struct LiveBlock {
    pub tree: ReactiveViewModel,
    /// Emits a new tree when the render expression changes (structural rebuild).
    /// Data-only changes do NOT emit — they update the existing tree in-place.
    pub structural_changes: Pin<Box<dyn futures::Stream<Item = ReactiveViewModel> + Send>>,
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::Value;

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
        let rq = ReactiveQueryResults::new();
        let interpret = Arc::new(test_interpret);
        let vm = poll_signal!(rq.reactive_signal(interpret));
        let debug = debug_tag(&vm);
        assert_eq!(debug, "loading:0");
    }

    #[tokio::test]
    async fn structure_event_sets_render_expr() {
        let rq = ReactiveQueryResults::new();
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
        let rq = ReactiveQueryResults::new();
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
        let rq = ReactiveQueryResults::new();
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
        let rq = ReactiveQueryResults::new();
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
        let rq = ReactiveQueryResults::new();
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
        let rq = ReactiveQueryResults::new();
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

        // Verify rows are in the BTreeMap (row_signal_vec tested via ReactiveCollection)
        let (_, rows) = rq.snapshot();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("id").unwrap().as_string().unwrap(), "a");
        assert_eq!(rows[1].get("id").unwrap().as_string().unwrap(), "b");
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
        use holon_api::render_types::{Arg, RenderExpr};

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
}
