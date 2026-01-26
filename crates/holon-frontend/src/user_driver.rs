//! `UserDriver` trait — frontend-agnostic abstraction for dispatching UI
//! mutations through the same code paths a real user exercises.
//!
//! `ReactiveEngineDriver` dispatches via `BuilderServices::dispatch_intent`
//! — the same path that GPUI click handlers and key-chord handlers use.
//! Also owns a `HeadlessInputRouter` that stores per-block content
//! snapshots for cross-block input routing.
//!
//! Frontend-specific drivers live alongside their frontend:
//!
//! - `DirectUserDriver` (in `holon-integration-tests`) — calls
//!   `BackendEngine::execute_operation` directly. Legacy PBT path.
//! - `GpuiUserDriver` (in `frontends/gpui`) — dispatches
//!   `InteractionEvent`s on the MCP `interaction_tx` channel. Works
//!   off-screen, doesn't touch the OS cursor.
//! - `FlutterUserDriver` (in `frontends/flutter`) — calls DartFnFuture
//!   callbacks.
//!
//! The `send_key_chord` method is the user-verb entry point — the way
//! tests simulate a real key press. The default impl uses
//! `bubble_input_oneshot` to DFS the tree and match keybindings.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use holon_api::{EntityName, EntityUri, KeyChord, Value};

use crate::input::{InputAction, WidgetInput};
use crate::operations::OperationIntent;
use crate::reactive::{BuilderServices, ReactiveEngine};
use crate::reactive_view_model::ReactiveViewModel;

/// Default operation dispatched when a drop completes on a block drop zone.
/// Used as fallback when `ViewKind::DropZone { op_name }` isn't readable.
pub const DEFAULT_DROP_OP_NAME: &str = "move_block";

/// Param key for the source block id on a drop dispatch.
pub const DROP_SOURCE_PARAM: &str = "id";

/// Param key for the target (new parent) block id on a drop dispatch.
pub const DROP_TARGET_PARAM: &str = "parent_id";

/// Build the `OperationIntent` that a drop zone widget dispatches when a
/// drag is released on it. Production GPUI `drop_zone` and the headless
/// `UserDriver::drop_entity` default impl both call this. `op_name` comes
/// from the dropzone widget's declarative spec (see
/// `ViewKind::DropZone { op_name }`).
pub fn build_drop_intent(
    source_id: &str,
    target_id: &str,
    target_entity: EntityName,
    op_name: &str,
) -> OperationIntent {
    let mut params = HashMap::new();
    params.insert(DROP_SOURCE_PARAM.into(), Value::String(source_id.into()));
    params.insert(DROP_TARGET_PARAM.into(), Value::String(target_id.into()));
    OperationIntent::new(target_entity, op_name.into(), params)
}

use crate::focus_path::walk_tree;

/// How UI mutations are dispatched to the system under test.
///
/// Backend PBTs use `ReactiveEngineDriver` (same path as GPUI).
/// Flutter tests provide a `FlutterUserDriver` that calls Dart callbacks
/// which drive WidgetTester interactions.
#[async_trait::async_trait]
pub trait UserDriver: Send + Sync {
    /// Synthetic dispatch — directly execute a UI operation without going
    /// through the key-chord / click / focus pipeline. Prefer the user verbs
    /// `send_key_chord` / `click_entity` / `type_text` whenever a real
    /// gesture exists.
    ///
    /// Legitimate uses:
    /// - PBT fuzz targets (e.g. `block::update` with random content) that
    ///   have no corresponding keybinding and are synthetic by design.
    /// - Concurrent-mutation race tests whose timing depends on synchronous
    ///   dispatch.
    /// - Fallbacks when a native UI driver couldn't handle an input.
    /// - Flutter FFI entry where the Dart side hasn't wrapped the user
    ///   verbs yet.
    ///
    /// If you're reaching for this because a test is easier to write
    /// synthetically than through the real pipeline, stop — either add a
    /// keybinding or use `type_text` / `click_entity`.
    ///
    /// Formerly named `apply_ui_mutation`, renamed in plan
    /// `deep-humming-crane.md` F10 to make the intent explicit.
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()>;

    /// Dispatch an `OperationIntent` — convenience wrapper around
    /// `synthetic_dispatch`.
    async fn apply_intent(&self, intent: OperationIntent) -> Result<()> {
        self.synthetic_dispatch(intent.entity_name.as_str(), &intent.op_name, intent.params)
            .await
    }

    /// Send a key chord on a focused entity.
    ///
    /// Default impl: DFS the `ReactiveViewModel` tree to find the entity,
    /// bubble the chord through ancestors, and dispatch the matched
    /// operation via `synthetic_dispatch`. `ReactiveEngineDriver` and
    /// native drivers override this to use their native input pipelines.
    ///
    /// `extra_params` is the canonical channel for UI-observable context
    /// that the chord resolver can't read (today: `split_block` cursor
    /// byte — mirrors the hardcoded injection at
    /// `frontends/gpui/src/lib.rs:670-702`). Drivers that synthesize
    /// real OS or channel input cannot thread this through the window
    /// pipeline, so chord dispatch falls through to a focus-path that
    /// injects `extra_params` into the matched operation's params.
    /// This is NOT a fallback — it is the intended path for that feature.
    ///
    /// TODO(simulate-real-input): the headless default impl below short-circuits
    /// `synthetic_dispatch` instead of going through the actual editor view's
    /// `capture_action(Enter)` / InputState pipeline. That hides bugs like
    /// "InputState swallows Enter as multi-line newline insertion." A truer
    /// headless equivalent would mount the same EditorController + on_key
    /// state machine the GPUI frontend uses, so chord dispatch traverses the
    /// editor before reaching the operation dispatcher.
    ///
    /// Returns `true` if the chord matched an operation and was dispatched.
    async fn send_key_chord(
        &self,
        root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        let action = crate::focus_path::bubble_input_oneshot(root_tree, entity_id, &input);
        match action {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut params = HashMap::new();
                params.insert("id".into(), Value::String(entity_id));
                params.extend(extra_params);
                self.synthetic_dispatch(&entity_name, &operation.name, params)
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Resolve a key chord on a focused entity without dispatching. Used by
    /// the SUT's `assert_keychord_resolves` diagnostic. Returns the resolved
    /// operation name if the chord matched, `None` otherwise.
    fn resolve_key_chord(
        &self,
        root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
    ) -> Option<String> {
        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        match crate::focus_path::bubble_input_oneshot(root_tree, entity_id, &input) {
            Some(InputAction::ExecuteOperation { operation, .. }) => Some(operation.name),
            _ => None,
        }
    }

    /// Click an entity — analogous to a mouse-down + mouse-up on the
    /// rendered element. Default impl dispatches the `navigation::editor_focus`
    /// intent with `region` and `cursor_offset=0`, matching what
    /// `frontends/gpui/src/render/builders/render_entity.rs` does inside the
    /// real GPUI click handler. Native drivers override this to synthesize
    /// real mouse input.
    async fn click_entity(&self, entity_id: &str, region: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".into(), Value::String(region.to_string()));
        params.insert("block_id".into(), Value::String(entity_id.to_string()));
        params.insert("cursor_offset".into(), Value::Integer(0));
        self.synthetic_dispatch("navigation", "editor_focus", params)
            .await
    }

    /// Tree-aware click — finds the node bound to `entity_id` in the live
    /// reactive tree and dispatches its `click_intent()` if the node has a
    /// click-triggered operation (e.g. a sidebar `selectable` whose
    /// `action: navigation_focus(...)` was wired by the shadow builder).
    ///
    /// Falls back to `click_entity` (which dispatches `navigation.editor_focus`,
    /// matching `render_entity`'s GPUI click handler) when the targeted node
    /// has no click action — that's the right behavior for clicking inside
    /// a main-panel block where the user just wants to place the editor cursor.
    /// `region` is the region the click happened in; needed by the
    /// `editor_focus` fallback (the bound-action path ignores it because the
    /// region is already in `bound_params`).
    ///
    /// Returns `true` if a bound click action was dispatched, `false` if the
    /// fallback `editor_focus` path was used. Errors propagate dispatch failures.
    async fn click_entity_with_tree(
        &self,
        _root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        region: &str,
    ) -> Result<bool> {
        if let Some(intent) = crate::focus_path::find_click_intent_oneshot(root_tree, entity_id) {
            self.apply_intent(intent).await?;
            return Ok(true);
        }
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    /// Replace the content of an entity with the given text — the headless
    /// equivalent of "focus the editor and type". Default impl dispatches
    /// `block::update { id, content: text }`. Native drivers override this
    /// to synthesize real key-by-key input, exercising the full IME /
    /// focus / editor pipeline.
    ///
    /// Note: this is a content replacement, not an append. The real user
    /// path at `frontends/gpui/src/lib.rs:670-702` goes through the focused
    /// editor state which reads/writes cursor-position-dependent content.
    /// The simplification is explicit — native drivers override this
    /// method to exercise the full editor pipeline.
    async fn type_text(&self, entity_id: &str, text: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(entity_id.to_string()));
        params.insert("content".into(), Value::String(text.to_string()));
        self.synthetic_dispatch("block", "update", params).await
    }

    /// Scroll at a window coordinate. `dx`/`dy` are scroll-wheel line deltas
    /// (positive `dy` = scroll down, positive `dx` = scroll right). Default
    /// impl is a no-op — headless drivers have no viewport. Native drivers
    /// override this to synthesize real scroll-wheel input.
    async fn scroll_at(&self, _x: f32, _y: f32, _dx: f32, _dy: f32) -> Result<()> {
        Ok(())
    }

    /// Scroll over a rendered entity — analogous to moving the mouse over
    /// the element and turning the wheel. Default impl is a no-op. Native
    /// drivers look up the element's screen position via their geometry
    /// provider and delegate to `scroll_at` with the element's center.
    async fn scroll_entity(&self, _entity_id: &str, _dx: f32, _dy: f32) -> Result<()> {
        Ok(())
    }

    /// Drag `source_id` onto `target_id` — analogous to a real
    /// click-hold-drag-release gesture. No default impl: each driver
    /// supplies its own simulation path (geometry-driven for GPUI, shadow
    /// tree walk for headless). Drivers without a real drag pipeline must
    /// fail loud rather than silently dispatching an unverified intent.
    ///
    /// `root_block_id` is the layout root that the test thinks is currently
    /// rendered — the headless driver bootstraps its router subscription
    /// from this. Native drivers ignore it.
    ///
    /// Returns `true` if the drop was dispatched. Errors propagate when the
    /// source isn't draggable or no drop zone exists for the target.
    async fn drop_entity(
        &self,
        root_block_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> Result<bool>;

    /// Send a single keystroke through the platform input pipeline. Used by
    /// the PBT's atomic editor primitives (`MoveCursor`, `TypeChars`,
    /// `DeleteBackward`, `PressKey`, `Blur`) so that each user gesture
    /// reaches the editor's `capture_action` / `InputState` pipeline the
    /// same way a real keypress would.
    ///
    /// `keystroke` is a GPUI-style key name (e.g. `"home"`, `"right"`,
    /// `"a"`, `"backspace"`, `"enter"`, `"escape"`). `modifiers` is a list
    /// of modifier names like `"cmd"` / `"ctrl"` / `"alt"` / `"shift"`.
    ///
    /// Default impl is `unimplemented!` — headless drivers have no
    /// `InputState`, so the bug class these primitives target (in-memory-
    /// vs-DB content divergence) doesn't exist there. Tests that use these
    /// primitives must run against a real-input driver (e.g. `GpuiUserDriver`).
    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        let _ = (keystroke, modifiers);
        anyhow::bail!(
            "send_raw_keystroke is unimplemented for this UserDriver. \
             Atomic editor primitives need a real-input driver (GpuiUserDriver). \
             Was PBT_ATOMIC_EDITOR=1 set in a headless run?"
        )
    }
}

/// Dispatches mutations via `BuilderServices::dispatch_intent` — the same
/// code path that GPUI click handlers and key-chord handlers use.
///
/// Also owns a `HeadlessInputRouter` that stores per-block content
/// snapshots for cross-block input routing.
pub struct ReactiveEngineDriver {
    engine: Arc<ReactiveEngine>,
    router: Arc<HeadlessInputRouter>,
}

impl ReactiveEngineDriver {
    pub fn new(engine: Arc<ReactiveEngine>) -> Self {
        let router = HeadlessInputRouter::new(engine.clone());
        Self { engine, router }
    }
}

#[async_trait::async_trait]
impl UserDriver for ReactiveEngineDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    async fn send_key_chord(
        &self,
        root_block_id: &str,
        _root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        // Establish the router's drain tasks (root + recursively-watched
        // descendants) and wait for the first emission to land BEFORE
        // building the focus path. The router warms `block_contents` as
        // each `live_block` becomes visible; without this barrier, fresh
        // descendant watchers return empty rows from `snapshot_reactive`
        // and the focus path can't find blocks rendered through nested
        // queries (main panel rows, sidebar items).
        self.router.ensure_block_watch(root_block_id);
        self.router
            .wait_until_ready(Duration::from_secs(2))
            .await
            .context("block contents not populated within timeout")?;

        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };

        // Poll the router's cross-block focus path until either the entity
        // is reachable or we time out. The router auto-extends watches to
        // nested live_blocks via `process_emission`, but those emissions
        // are async — root emits first, then sidebars/main panel, then
        // their descendants. A bulk-added block (`block:bulk-0-7` in the
        // PBT) may live three levels deep, and `wait_until_ready` only
        // confirms root populated. Without the poll we race the chord
        // against the descendant fan-out.
        let deadline = Instant::now() + Duration::from_secs(2);
        let action = loop {
            if let Some(action) = self.router.bubble_input(entity_id, &input) {
                break Some(action);
            }
            if Instant::now() >= deadline {
                break None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        // Final fallback: if the router never saw the entity within the
        // poll window, build a fresh engine-snapshot focus path. This is
        // the same pattern GPUI uses for chord resolution when its router
        // is mid-fan-out, and it forces `ensure_watching` for every
        // live_block on the descent.
        let action = match action {
            Some(action) => Some(action),
            None => {
                let engine_for_resolver = self.engine.clone();
                let resolver: crate::focus_path::LiveBlockResolver =
                    Arc::new(move |block_id: &str| {
                        let uri = EntityUri::from_raw(block_id);
                        Some(Arc::new(engine_for_resolver.snapshot_reactive(&uri)))
                    });

                let root_uri = EntityUri::from_raw(root_block_id);
                let root_tree = Arc::new(self.engine.snapshot_reactive(&root_uri));
                let fp = crate::focus_path::build_focus_path_with_resolver(
                    &root_tree,
                    entity_id,
                    resolver.as_ref(),
                );
                if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
                    eprintln!(
                        "[CHORD-FALLBACK] router timeout for entity={} chord={:?}; \
                         engine fp_found={}",
                        entity_id,
                        chord,
                        fp.is_some(),
                    );
                    if let Some(fp) = &fp {
                        eprintln!("  engine path ids: {:?}", fp.entity_ids());
                    }
                    eprintln!("  router state:\n{}", self.router.diagnostic_snapshot());
                }
                fp.and_then(|fp| fp.bubble_input(entity_id, &input))
            }
        };

        match action {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut params = HashMap::new();
                params.insert("id".into(), Value::String(entity_id));
                params.extend(extra_params);
                let tick_snapshot = self.router.current_tick();
                self.synthetic_dispatch(&entity_name, &operation.name, params)
                    .await?;
                let (window, timeout) = quiescence_config();
                self.router
                    .wait_for_quiescence(tick_snapshot, window, timeout)
                    .await
                    .context("emissions did not quiesce after dispatch — CDC pipeline stuck?")?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn resolve_key_chord(
        &self,
        root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
    ) -> Option<String> {
        // If the router's index is populated, use it; otherwise fall back to
        // the trait default (build a fresh index from root_tree).
        if self.router.is_ready() {
            let input = WidgetInput::KeyChord {
                keys: chord.0.clone(),
            };
            match self.router.bubble_input(entity_id, &input) {
                Some(InputAction::ExecuteOperation { operation, .. }) => {
                    return Some(operation.name);
                }
                _ => return None,
            }
        }
        // Fallback: one-shot DFS+bubble from the tree snapshot.
        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        match crate::focus_path::bubble_input_oneshot(root_tree, entity_id, &input) {
            Some(InputAction::ExecuteOperation { operation, .. }) => Some(operation.name),
            _ => None,
        }
    }

    /// Override walks the router's per-block content store rather than the
    /// passed `root_tree`. `snapshot_reactive` only resolves the root level —
    /// drop_zone / draggable widgets live inside nested blocks, which the
    /// router has been keeping warm since `ensure_block_watch`.
    ///
    /// Lazy population: `block_contents` fills incrementally as `live_block`
    /// slots resolve their nested trees. The reference state may pick a
    /// source block that exists in the focus tree before its router entry
    /// has populated. Poll until the Draggable for `source_id` AND the
    /// DropZone for `target_id` appear, then dispatch. Bail loud on
    /// timeout — that means the source/target was never rendered and the
    /// gesture would have been impossible for a real user.
    async fn drop_entity(
        &self,
        root_block_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> Result<bool> {
        // Bootstrap router on the layout root. `send_key_chord` does this
        // too — without it, drop_entity sees an empty router when it's the
        // first user verb after StartApp.
        self.router.ensure_block_watch(root_block_id);
        self.router
            .wait_until_ready(Duration::from_secs(2))
            .await
            .context("router not ready before drop_entity")?;

        let deadline = Instant::now() + drop_widget_timeout();
        let (entity, op) = loop {
            let mut found_source = false;
            let mut target_entity: Option<EntityName> = None;
            let mut target_op: Option<String> = None;
            {
                let contents = self.router.block_contents.lock().unwrap();
                for tree in contents.values() {
                    walk_tree(tree, &mut |n| {
                        if !found_source
                            && n.widget_name().as_deref() == Some("draggable")
                            && n.row_id().as_deref() == Some(source_id)
                        {
                            found_source = true;
                        }
                        if target_entity.is_none()
                            && n.widget_name().as_deref() == Some("drop_zone")
                            && n.row_id().as_deref() == Some(target_id)
                        {
                            target_entity =
                                Some(n.entity_name().unwrap_or_else(|| EntityName::new("block")));
                            target_op = Some(
                                n.prop_str("op")
                                    .or_else(|| n.prop_str("op_name"))
                                    .unwrap_or_else(|| DEFAULT_DROP_OP_NAME.to_string()),
                            );
                        }
                    });
                    if found_source && target_entity.is_some() {
                        break;
                    }
                }
            }
            if let (true, Some(entity)) = (found_source, target_entity) {
                break (
                    entity,
                    target_op.unwrap_or_else(|| DEFAULT_DROP_OP_NAME.to_string()),
                );
            }
            if Instant::now() >= deadline {
                let diag = self.router.diagnostic_snapshot();
                if !found_source {
                    anyhow::bail!(
                        "drop_entity: no Draggable widget covers source block {source_id} \
                         after {:?} — the source's block tree never populated in the \
                         router (live_block slot didn't resolve, or the block's render \
                         template doesn't include `draggable(...)`).\n\
                         Router diagnostic:\n{diag}",
                        drop_widget_timeout()
                    );
                }
                anyhow::bail!(
                    "drop_entity: no DropZone widget renders for target block {target_id} \
                     after {:?} — the target's block tree never populated in the router \
                     (live_block slot didn't resolve, or the block's render template \
                     doesn't include `drop_zone(...)`).\n\
                     Router diagnostic:\n{diag}",
                    drop_widget_timeout()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };

        let tick_snapshot = self.router.current_tick();
        let intent = build_drop_intent(source_id, target_id, entity, &op);
        self.apply_intent(intent).await?;
        let (window, timeout) = quiescence_config();
        self.router
            .wait_for_quiescence(tick_snapshot, window, timeout)
            .await
            .context("emissions did not quiesce after drop dispatch — CDC pipeline stuck?")?;
        Ok(true)
    }
}

// ── HeadlessInputRouter ───────────────────────────────────────────────

/// Per-block content store for headless tests.
///
/// Stores a `HashMap<block_id, Arc<ReactiveViewModel>>` updated by
/// per-block drain tasks. Uses `build_focus_path_cross_block` for
/// `bubble_input` — no flattened index, no splice/shift bookkeeping.
struct HeadlessInputRouter {
    engine: Arc<ReactiveEngine>,
    /// Per-block content snapshots. Updated by drain tasks on each emission.
    block_contents: Arc<Mutex<HashMap<String, Arc<ReactiveViewModel>>>>,
    /// block_id → drain task handle.
    watches: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Set when the first emission has been applied.
    ready: Arc<tokio::sync::Notify>,
    /// Root block id, established on the first `ensure_block_watch` call.
    root_block_id: OnceLock<String>,
    /// F2: monotonic counter bumped after every emission. Readers snapshot,
    /// trigger a dispatch, then wait until the counter advances past the
    /// snapshot and stabilizes.
    last_patch_tick: AtomicU64,
    /// F_drop: cancellation notifier for drain tasks.
    cancel: Arc<tokio::sync::Notify>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

/// Quiescence window for F2 — how long the router must remain silent after
/// a dispatched mutation before we consider it settled. 20ms default,
/// override via `HOLON_PBT_QUIESCENCE_MS` for slower CI.
const DEFAULT_QUIESCENCE_MS: u64 = 20;
const DEFAULT_QUIESCENCE_TIMEOUT_MS: u64 = 2000;

/// How long `drop_entity` polls `block_contents` for the source/target
/// widgets before bailing. Override via `HOLON_PBT_DROP_TIMEOUT_MS`.
const DEFAULT_DROP_WIDGET_TIMEOUT_MS: u64 = 5000;

fn drop_widget_timeout() -> Duration {
    let ms = std::env::var("HOLON_PBT_DROP_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_DROP_WIDGET_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn quiescence_config() -> (Duration, Duration) {
    let window = std::env::var("HOLON_PBT_QUIESCENCE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_QUIESCENCE_MS);
    let timeout = std::env::var("HOLON_PBT_QUIESCENCE_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_QUIESCENCE_TIMEOUT_MS);
    (
        Duration::from_millis(window),
        Duration::from_millis(timeout),
    )
}

impl HeadlessInputRouter {
    fn new(engine: Arc<ReactiveEngine>) -> Arc<Self> {
        Arc::new(Self {
            engine,
            block_contents: Arc::new(Mutex::new(HashMap::new())),
            watches: Arc::new(Mutex::new(HashMap::new())),
            ready: Arc::new(tokio::sync::Notify::new()),
            root_block_id: OnceLock::new(),
            last_patch_tick: AtomicU64::new(0),
            cancel: Arc::new(tokio::sync::Notify::new()),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    fn is_ready(&self) -> bool {
        let contents = self.block_contents.lock().unwrap();
        self.root_block_id
            .get()
            .map(|r| contents.contains_key(r.as_str()))
            .unwrap_or(false)
    }

    fn current_tick(&self) -> u64 {
        self.last_patch_tick.load(Ordering::Acquire)
    }

    async fn wait_until_ready(&self, timeout: Duration) -> Result<()> {
        if self.is_ready() {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("timed out waiting for block contents to populate");
            }
            let notified = self.ready.notified();
            tokio::select! {
                _ = notified => {
                    if self.is_ready() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    anyhow::bail!("timed out waiting for block contents to populate");
                }
            }
        }
    }

    /// F2: approximate post-dispatch barrier. Waits for the emission tick to
    /// advance past `snapshot` and then stay stable for `window`.
    async fn wait_for_quiescence(
        &self,
        snapshot: u64,
        window: Duration,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while self.current_tick() == snapshot {
            if Instant::now() >= deadline {
                anyhow::bail!("emissions did not advance past tick {snapshot} within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        loop {
            let before = self.current_tick();
            tokio::time::sleep(window).await;
            if self.current_tick() == before {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("emissions did not quiesce (kept advancing) within {timeout:?}");
            }
        }
    }

    /// Human-readable summary of what the router has populated. Used by
    /// `drop_entity`'s timeout error so the test log shows exactly which
    /// blocks resolved, which widgets they contain, and what was missing.
    fn diagnostic_snapshot(&self) -> String {
        let contents = self.block_contents.lock().unwrap();
        let watches = self.watches.lock().unwrap();
        let watched: Vec<_> = watches.keys().cloned().collect();
        let populated: Vec<_> = contents.keys().cloned().collect();
        let mut row_ids_per_widget: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (block_id, tree) in contents.iter() {
            walk_tree(tree, &mut |n| {
                let Some(name) = n.widget_name() else { return };
                if matches!(name.as_str(), "draggable" | "drop_zone" | "live_block") {
                    let row = n.row_id().unwrap_or_else(|| "<no row_id>".into());
                    row_ids_per_widget
                        .entry(name)
                        .or_default()
                        .push((block_id.clone(), row));
                }
            });
        }
        let mut s = String::new();
        s.push_str(&format!(
            "  watches      ({}): {watched:?}\n",
            watched.len()
        ));
        s.push_str(&format!(
            "  populated    ({}): {populated:?}\n",
            populated.len()
        ));
        for (widget, entries) in &row_ids_per_widget {
            s.push_str(&format!(
                "  widget {widget:>11} ({}): {entries:?}\n",
                entries.len()
            ));
        }
        s
    }

    fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        let contents = self.block_contents.lock().unwrap();
        let root_id = self.root_block_id.get()?;
        let root_content = contents.get(root_id.as_str())?;
        let fp =
            crate::focus_path::build_focus_path_cross_block(root_content, &contents, entity_id)?;
        fp.bubble_input(entity_id, input)
    }

    fn ensure_block_watch(self: &Arc<Self>, block_id: &str) {
        let _ = self.root_block_id.set(block_id.to_string());

        {
            let watches = self.watches.lock().unwrap();
            if watches.contains_key(block_id) {
                return;
            }
        }

        let block_uri = EntityUri::from_raw(block_id);
        let stream = self.engine.watch(&block_uri);
        let router_weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        let cancelled = self.cancelled.clone();
        let bid = block_id.to_string();

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = stream;
            loop {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let notified = cancel.notified();
                tokio::select! {
                    maybe_rvm = stream.next() => {
                        let Some(rvm) = maybe_rvm else { break };
                        let Some(router) = router_weak.upgrade() else { break };
                        let rvm_arc = Arc::new(rvm);
                        router.process_emission(&bid, rvm_arc);
                        drop(router);
                    }
                    _ = notified => break,
                }
            }
        });

        self.watches
            .lock()
            .unwrap()
            .insert(block_id.to_string(), handle);
    }

    fn process_emission(self: &Arc<Self>, block_id: &str, rvm: Arc<ReactiveViewModel>) {
        let was_first_root = {
            let mut contents = self.block_contents.lock().unwrap();
            let is_root = self
                .root_block_id
                .get()
                .map(|r| r == block_id)
                .unwrap_or(false);

            if contents.is_empty() && !is_root {
                tracing::debug!(
                    block_id,
                    "process_emission: dropping pre-root nested emission"
                );
                return;
            }

            let was_empty = contents.is_empty();
            contents.insert(block_id.to_string(), rvm.clone());
            was_empty
        };

        self.last_patch_tick.fetch_add(1, Ordering::AcqRel);

        let mut nested = HashSet::new();
        collect_nested_block_refs(&rvm, &mut nested);
        for nested_id in &nested {
            if nested_id == block_id {
                continue;
            }
            self.ensure_block_watch(nested_id);
        }

        if was_first_root {
            self.ready.notify_waiters();
        }
    }
}

impl Drop for HeadlessInputRouter {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.cancel.notify_waiters();
        self.watches.lock().unwrap().clear();
    }
}

/// Walk a `ReactiveViewModel` tree to discover direct `LiveBlock` children
/// (stops at LiveBlock boundaries — does not recurse into their slots).
fn collect_nested_block_refs(node: &ReactiveViewModel, out: &mut HashSet<String>) {
    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            out.insert(block_id.to_string());
        }
        return;
    }

    for child in &node.children {
        collect_nested_block_refs(child, out);
    }

    if let Some(ref view) = node.collection {
        let items: Vec<_> = view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            collect_nested_block_refs(item, out);
        }
    }

    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        collect_nested_block_refs(&guard, out);
    }
}
