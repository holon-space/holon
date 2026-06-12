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
/// Used as fallback when `ViewKind::DropZone { op_name }` isn't readable. // ALLOW(fallback): doc comment describing pre-existing default-op semantics
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
    source_id: &EntityUri,
    target_id: &EntityUri,
    target_entity: EntityName,
    op_name: &str,
) -> OperationIntent {
    let mut params = HashMap::new();
    params.insert(
        DROP_SOURCE_PARAM.into(),
        Value::String(source_id.to_string()),
    );
    params.insert(
        DROP_TARGET_PARAM.into(),
        Value::String(target_id.to_string()),
    );
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
    /// No default impl: each driver must implement input dispatch in its
    /// own medium. Headless drivers (`ReactiveEngineDriver`,
    /// `DirectUserDriver`) DFS the `ReactiveViewModel` tree, bubble the
    /// chord through ancestors, and `synthetic_dispatch` the matched
    /// operation. Screen drivers (GPUI, TUI) route the chord through their
    /// real input pipeline (`InteractionEvent` channel, `crossterm` event,
    /// etc.) so the keystroke traverses the editor's `capture_action` /
    /// `InputState` machinery before reaching dispatch.
    ///
    /// `extra_params` is the canonical channel for UI-observable context
    /// that the chord resolver can't read (today: `split_block` cursor
    /// byte — mirrors the hardcoded injection at
    /// `frontends/gpui/src/lib.rs:670-702`). Drivers that synthesize
    /// real OS or channel input cannot thread this through the window
    /// pipeline, so chord dispatch falls through to a focus-path that
    /// injects `extra_params` into the matched operation's params.
    /// This is NOT a fallback — it is the intended path for that feature. // ALLOW(fallback): doc explicitly says "NOT a fallback"
    ///
    /// Returns `true` if the chord matched an operation and was dispatched.
    async fn send_key_chord(
        &self,
        root_block_id: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool>;

    /// Resolve a key chord on a focused entity without dispatching. Used by
    /// the SUT's `assert_keychord_resolves` diagnostic. Returns the resolved
    /// operation name if the chord matched, `None` otherwise.
    fn resolve_key_chord(
        &self,
        // Part of the UserDriver contract (callers pass the layout root); no
        // resolve impl consumes it — they key off the router / root_tree.
        _: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
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
    /// rendered element. Screen drivers (GPUI, TUI) synthesize real input
    /// at the entity's coordinates so the click traverses the
    /// production click handler chain (`selectable.on_mouse_down`,
    /// `render_entity` cursor-placement). Headless drivers dispatch the
    /// bound `click_intent` (or `navigation.editor_focus` when none).
    ///
    /// No default impl by design: the trait must not silently launder a
    /// `synthetic_dispatch` shortcut into a screen-mode driver where the
    /// user couldn't actually reach the action without going through the
    /// click handler. See [`UserDriver`] doc block on medium-faithfulness.
    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<()>;

    /// Tree-aware click — same gesture as `click_entity`, but with access
    /// to a `ReactiveViewModel` root the headless driver can use to
    /// resolve the bound click intent before dispatch. Screen drivers
    /// ignore the tree (their click handler reads `click_intent` itself
    /// at the rendered widget) and delegate to `click_entity`.
    ///
    /// Returns `true` iff the driver could establish that a bound click
    /// action was dispatched. Screen drivers return `false` because they
    /// can't synchronously prove which intent fired — callers needing
    /// post-click state must observe through the SUT.
    async fn click_entity_with_tree(
        &self,
        root_block_id: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<bool>;

    /// Replace `entity_id`'s content with `text` by driving the *real* editor:
    /// focus it (a click, as `FocusEditableText` does), clear the existing
    /// content caret-wise, then type the replacement one codepoint at a time —
    /// every step a `send_raw_keystroke`, so the edit traverses the same editor
    /// pipeline (cursor model, `MutableText`, `on_text_changed`, the structural
    /// `split`/`join` decisions in `structural_block_action`) a user's typing
    /// would. This is the faithful replacement for `synthetic_dispatch`-ing a
    /// `block::update { content }`.
    ///
    /// The default impl composes the medium-agnostic real-input verbs, so it is
    /// correct for both headless (`send_raw_keystroke` → `HeadlessEditorMirror`)
    /// and screen drivers (real `InputState`) without per-driver code — unlike
    /// the synthetic `type_text`, it cannot launder a shortcut past the editor.
    async fn replace_text(&self, entity_id: &EntityUri, text: &str) -> Result<()> {
        // Focus the editor the same way `FocusEditableText` does.
        self.click_entity(entity_id, "main").await?;
        // Clear existing content: caret to end, then one backspace per char.
        // Each backspace is a real `MutableText` delete; we stop exactly at
        // caret 0 so the final keystroke doesn't fall through to `join_block`.
        let existing = self.displayed_text(entity_id).ok_or_else(|| {
            anyhow::anyhow!("replace_text: {entity_id} is not rendered — no content to replace")
        })?;
        self.send_raw_keystroke("end", &[]).await?;
        for _ in existing.chars() {
            self.send_raw_keystroke("backspace", &[]).await?;
        }
        // Type the replacement one codepoint at a time.
        for ch in text.chars() {
            self.send_raw_keystroke(&ch.to_string(), &[]).await?;
        }
        Ok(())
    }

    // ── Observation verbs ──────────────────────────────────────────────
    //
    // The test asks the driver "what's visible / what's reachable / what
    // does clicking here do" — the driver answers in its own medium.
    // Screen drivers consult their bounds registry; headless walks the
    // ViewModel tree. PBT generators talk only to these verbs and never
    // peek into geometry or VM trees directly, so the medium difference
    // stays inside the driver.

    /// True if `entity_id` is currently visible to the user — has rendered
    /// bounds with non-zero area (screen drivers) or appears in the
    /// rendered ViewModel tree (headless).
    ///
    /// Sync — observation verbs are point-in-time reads of state the
    /// driver already maintains (BoundsRegistry, VM-tree snapshot).
    /// Generators are sync, so all observation must be sync too; if a
    /// future impl needs to wait for state to settle, expose a separate
    /// barrier verb and have the generator call it explicitly.
    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool;

    /// Byte offset of the editor caret tracked for `block_id`, when this
    /// driver's medium exposes one. `Err(reason)` = caret unobservable in
    /// this medium (default; a disclosed skip, not a silent pass — GPUI's
    /// caret lives in window-local `InputState`). `Ok(None)` = observable
    /// medium but no caret tracked for this block yet (no keystroke since
    /// focus). Used by `inv-editor-caret-matches-ref`.
    fn editor_cursor_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        let _ = block_id;
        Err("editor caret not observable by this driver".to_string())
    }

    /// True iff `entity_id` is visible AND located within `region`'s panel.
    fn is_in_region(&self, entity_id: &EntityUri, region: holon_api::Region) -> bool;

    /// Entity ids currently visible in `region`'s panel. Use this when you
    /// want the actually-on-screen set (post-scroll, post-viewport-cull).
    fn entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri>;

    /// Entity ids the user could reach in `region` by scrolling — a
    /// superset of `entities_in_region`. PBT generators pick targets from
    /// this set and then call `scroll_to_entity` before clicking, so the
    /// scroll mechanism itself is part of the user verb under test.
    fn reachable_entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri>;

    /// Bring `entity_id` into the user-visible viewport via a real user
    /// action (scroll the parent scrollable). Returns `Ok(())` whether or
    /// not the entity was reached — caller must follow up with
    /// `is_widget_visible` to confirm (catches "last items can't be
    /// scrolled to" regressions). Headless is a no-op (no viewport).
    ///
    /// Stays `async` because this is an action verb — screen drivers
    /// dispatch real scroll input that must be awaited.
    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<()>;

    /// The click intent bound to `entity_id` if the rendered widget has
    /// one (e.g. a `selectable` whose action is `navigation_focus(...)`).
    /// Read-only — does not dispatch. Used by the generator to decide
    /// "would clicking here do what I want?"
    fn click_intent_of(&self, entity_id: &EntityUri) -> Option<OperationIntent>;

    /// The text the user actually sees rendered for `entity_id`. Screen
    /// drivers return the live displayed text; headless returns the
    /// ViewModel's resolved content. None for widgets without textual
    /// content.
    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String>;

    /// Scroll at a window coordinate. `dx`/`dy` are scroll-wheel line deltas
    /// (positive `dy` = scroll down, positive `dx` = scroll right). Default
    /// impl is a no-op — headless drivers have no viewport. Native drivers
    /// override this to synthesize real scroll-wheel input.
    async fn scroll_at(&self, _: f32, _: f32, _: f32, _: f32) -> Result<()> {
        Ok(())
    }

    /// Scroll over a rendered entity — analogous to moving the mouse over
    /// the element and turning the wheel. Default impl is a no-op. Native
    /// drivers look up the element's screen position via their geometry
    /// provider and delegate to `scroll_at` with the element's center.
    async fn scroll_entity(&self, _: &EntityUri, _: f32, _: f32) -> Result<()> {
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
        root_block_id: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
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
             Was an atomic-editor transition generated for a headless run \
             (the editor buffer capability requires a real-input driver)?"
        )
    }

    /// Like [`UserDriver::send_raw_keystroke`], but retry until some handler
    /// consumes the keystroke or `timeout` elapses. Real-window drivers
    /// override this to cover the editor-mount race: after a focus move, the
    /// view that will consume the key may mount only on a later render pass
    /// (and in a virtualized list, only after the focused row scrolls into
    /// view). Headless drivers consume synchronously — the default just
    /// forwards.
    async fn send_raw_keystroke_until_handled(
        &self,
        keystroke: &str,
        modifiers: &[&str],
        timeout: std::time::Duration,
    ) -> Result<()> {
        let _ = timeout;
        self.send_raw_keystroke(keystroke, modifiers).await
    }

    /// Whether `send_raw_keystroke` routes through a real input pipeline
    /// that performs key-chord resolution before any keystroke reaches an
    /// editor (TUI / GPUI native drivers). Headless drivers
    /// (`ReactiveEngineDriver`, `DirectUserDriver`) return `false` —
    /// their `send_raw_keystroke` writes straight into the focused
    /// editor's `MutableText` mirror, so a leader chord like `SPC b`
    /// would TYPE " b" into the editor instead of dispatching `go_back`.
    /// PBT helpers like `send_leader_chord` consult this to decide
    /// whether to emit raw keystrokes (chord-routed) or call
    /// `synthetic_dispatch` directly.
    fn dispatches_chords_via_raw_keystroke(&self) -> bool {
        false
    }
}

/// Dispatches mutations via `BuilderServices::dispatch_intent` — the same
/// code path that GPUI click handlers and key-chord handlers use.
///
/// Also owns a `HeadlessInputRouter` that stores per-block content
/// snapshots for cross-block input routing, and a `HeadlessEditorMirror`
/// that lets `send_raw_keystroke` simulate per-keystroke typing through
/// `MutableText`/Loro the same way `gpui-component`'s `InputState` does
/// in production GPUI.
pub struct ReactiveEngineDriver {
    engine: Arc<ReactiveEngine>,
    router: Arc<HeadlessInputRouter>,
    editor_mirror: Arc<crate::headless_editor_mirror::HeadlessEditorMirror>,
}

impl ReactiveEngineDriver {
    pub fn new(engine: Arc<ReactiveEngine>) -> Self {
        let router = HeadlessInputRouter::new(engine.clone());
        let editor_mirror = Arc::new(crate::headless_editor_mirror::HeadlessEditorMirror::new());
        Self {
            engine,
            router,
            editor_mirror,
        }
    }

    /// Ensure the router is warmed for `root_block_id`. Idempotent — safe to
    /// call before every chord. Used by per-frontend drivers that route input
    /// through the real UI pipeline but still need the engine-quiescence
    /// barrier (`tui` `TuiUserDriver`).
    pub async fn warm_for_block(&self, root_block_id: &EntityUri) -> Result<()> {
        self.router.ensure_block_watch(root_block_id);
        self.router
            .wait_until_ready(Duration::from_secs(2))
            .await
            .context("block contents not populated within timeout")
    }

    /// Snapshot the engine's emission tick. Pair with
    /// [`wait_emissions_quiesced`] to barrier on CDC settling. Wrapper around
    /// the same `current_tick`/`wait_for_quiescence` pair used internally by
    /// `send_key_chord`'s post-dispatch barrier.
    pub fn snapshot_emission_tick(&self) -> u64 {
        self.router.current_tick()
    }

    /// Wait until the emission tick has advanced past `snapshot` AND then
    /// stayed stable for the configured quiescence window. Returns an error
    /// (per project's fail-loud policy) if the pipeline never advances or
    /// keeps churning past `timeout`.
    pub async fn wait_emissions_quiesced(&self, snapshot: u64) -> Result<()> {
        let (window, timeout) = quiescence_config();
        self.router
            .wait_for_quiescence(snapshot, window, timeout)
            .await
            .context("emissions did not quiesce after dispatch — CDC pipeline stuck?")
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

    /// Mirror GPUI's `selectable` + `render_entity` click priority:
    /// dispatch the node's bound click intent if one exists; otherwise
    /// fall through to `navigation::editor_focus` (cursor placement).
    ///
    /// The bound-action path is the same one GPUI takes
    /// (`frontends/gpui/src/render/builders/selectable.rs:46-54` reads
    /// `node.click_intent()` and dispatches it from `on_mouse_down`).
    /// `BuilderServices::snapshot_resolved` recursively interprets every
    /// nested `live_block` so the resolved tree contains the
    /// sidebar/panel children where the bound action lives;
    /// `find_click_intent_in_view_model` then walks it.
    ///
    /// This keeps the headless and GPUI paths converging on the same
    /// click semantics: ViewModels carry the intent, drivers dispatch it.
    ///
    /// Poll for the entity in the resolved tree: nested `live_block`
    /// watches stream in async, so a click that lands immediately after
    /// `apply_start_app` may see an empty list. Same pattern
    /// `send_key_chord` uses for its router fallback. If the entity is // ALLOW(fallback): pre-existing doc on router-poll behavior
    /// never found, we fall through to cursor placement — same as GPUI
    /// when nothing intercepts the click.
    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<()> {
        let root_uri = holon_api::root_layout_block_uri();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let resolved = self.engine.snapshot_resolved(&root_uri);
            // Scope the intent lookup to the clicked region. The same
            // entity_id can appear in multiple panels (e.g. `block:journals`
            // is both a LeftSidebar list item and a Main-panel doc), and the
            // wrappers bind different actions per region. The unscoped
            // walker would return the LeftSidebar's `navigation.focus` for a
            // Main click and diverge from production GPUI semantics. See
            // FU-15 in `devlog/2026-05-07-164740-logseq-sidebar-followups.md`.
            if let Some(intent) =
                crate::focus_path::find_click_intent_in_region(&resolved, entity_id, region)
            {
                return self.apply_intent(intent).await;
            }
            // The entity is already rendered in this region but binds no
            // click-intent (e.g. an `editable_text` block in Main, where a
            // click just places the cursor / focuses). Polling cannot make an
            // intent materialise, so stop waiting and let the focus path below
            // run immediately — the same focus-on-click GPUI does for a block
            // with no bound action. Keep polling only while the entity is
            // still ABSENT, a genuine "tree not resolved yet" race. This turns
            // SplitBlock's per-click cost from a flat 2s deadline burn into a
            // single snapshot.
            if crate::focus_path::region_contains_entity(&resolved, entity_id, region) {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // No bound action found within the deadline — fall through to
        // focusing the block, matching GPUI's `render_entity` click handler.
        // Focus is pure in-memory state (ADR 0010): set the authority
        // directly instead of dispatching `navigation.editor_focus`.
        let _ = region;
        self.engine.set_focus(Some(entity_id.clone()));
        Ok(())
    }

    async fn send_key_chord(
        &self,
        root_block_id: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
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

        // Chord dispatch clicks the entity before pressing the chord (see
        // the GPUI driver), and that click re-opens the block's editor. Seed
        // the caret mirror like the resulting editor mount would — a cursor
        // tracked during an earlier editor session on this block is stale.
        // When the entity is ALREADY the focused editor no click happens
        // (the chord goes straight to the open editor, like a real user),
        // so the tracked caret must be left untouched — mirrors the
        // already-active early-return in the PBT ref's
        // `model_chord_click_focus`.
        if self.engine.focused_block().as_ref() != Some(entity_id) {
            self.editor_mirror
                .seed_for_click(&self.engine, entity_id)
                .await
                .with_context(|| format!("send_key_chord: caret seed for clicked {entity_id}"))?;
        }

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

        // Final fallback: if the router never saw the entity within the // ALLOW(fallback): pre-existing one-shot DFS escape hatch with explicit warn
        // poll window, build a fresh engine-snapshot focus path. This is
        // the same pattern GPUI uses for chord resolution when its router
        // is mid-fan-out, and it forces `ensure_watching` for every
        // live_block on the descent.
        let action = match action {
            Some(action) => Some(action),
            None => {
                let engine_for_resolver = self.engine.clone();
                let resolver: crate::focus_path::LiveBlockResolver =
                    Arc::new(move |block_id: &EntityUri| {
                        Some(Arc::new(engine_for_resolver.snapshot_reactive(block_id)))
                    });

                let root_tree = Arc::new(self.engine.snapshot_reactive(root_block_id));
                let fp = crate::focus_path::build_focus_path_with_resolver(
                    &root_tree,
                    entity_id,
                    resolver.as_ref(),
                );
                if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
                    eprintln!(
                        // ALLOW(fallback): debug message describing existing fallback path
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
                params.insert("id".into(), Value::String(entity_id.to_string()));
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
        // Part of the UserDriver contract (callers pass the layout root); no
        // resolve impl consumes it — they key off the router / root_tree.
        _: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
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
        // Fallback: one-shot DFS+bubble from the tree snapshot. // ALLOW(fallback): pre-existing one-shot escape hatch when router has no path
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
        root_block_id: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
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
                            && n.row_id().as_deref() == Some(source_id.as_str())
                        {
                            found_source = true;
                        }
                        if target_entity.is_none()
                            && n.widget_name().as_deref() == Some("drop_zone")
                            && n.row_id().as_deref() == Some(target_id.as_str())
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

    /// Headless per-keystroke routing. Mirrors the production GPUI editor:
    /// char keys mutate the focused block's `MutableText` in Loro one
    /// codepoint at a time; Enter / Backspace-at-0 / Tab / Shift+Tab
    /// dispatch their structural intents at the live cursor (same path
    /// `editor_view.rs:548-619` takes from `capture_action`).
    ///
    /// Without this override, atomic editor PBT primitives (`TypeChars`,
    /// `PressKey`, `DeleteBackward`, `MoveCursor`) bail at the trait
    /// default and the keystroke-driven bug surface — sync race between
    /// MutableText writes and `block.content` SQL projection — never
    /// reaches the headless pipeline.
    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        self.editor_mirror
            .handle_keystroke(&self.engine, keystroke, modifiers)
            .await
    }

    /// Headless caret observation: read the `HeadlessEditorMirror`'s tracked
    /// byte cursor (same map `send_raw_keystroke` advances). Keyed by the
    /// full URI string, matching `handle_keystroke`'s `block_uri.to_string()`.
    fn editor_cursor_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        Ok(self.editor_mirror.tracked_cursor(&block_id.to_string()))
    }

    /// Tree-aware click: dispatch the bound `click_intent()` if the node has
    /// one, else fall through to `click_entity` (cursor placement). Headless
    /// — synchronous resolution against the passed `root_tree`, no router
    /// warm-up needed because the caller already produced the tree.
    async fn click_entity_with_tree(
        &self,
        _: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<bool> {
        if let Some(intent) = crate::focus_path::find_click_intent_oneshot(root_tree, entity_id) {
            self.apply_intent(intent).await?;
            return Ok(true);
        }
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    // ── Observation verbs ──────────────────────────────────────────────
    //
    // Headless answers via VM-tree walk over the router's per-block
    // snapshots. The router warms watches as nested `live_block`s
    // resolve, so the panel's tree may take a moment to populate after
    // app start — callers that need synchronous answers must
    // `warm_for_block` first.

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        let contents = self.router.block_contents.lock().unwrap();
        contents.values().any(|tree| {
            let mut found = false;
            walk_tree(tree, &mut |n| {
                if !found && n.entity_id().as_ref() == Some(entity_id) {
                    found = true;
                }
            });
            found
        })
    }

    fn is_in_region(&self, entity_id: &EntityUri, region: holon_api::Region) -> bool {
        self.entities_in_region(region)
            .iter()
            .any(|uri| uri == entity_id)
    }

    fn entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        let panel_id = region_panel_block_id(region);
        let contents = self.router.block_contents.lock().unwrap();
        let Some(tree) = contents.get(&panel_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        walk_tree(tree, &mut |n| {
            if let Some(uri) = n.entity_id() {
                out.push(uri);
            }
        });
        out
    }

    /// Headless has no viewport, so "reachable by scrolling" == "in tree".
    /// Same answer as `entities_in_region`.
    fn reachable_entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        self.entities_in_region(region)
    }

    /// No-op: headless has no viewport. Tests that exercise scrolling as
    /// a user verb must run against a screen driver.
    async fn scroll_to_entity(&self, _: &EntityUri) -> Result<()> {
        Ok(())
    }

    fn click_intent_of(&self, entity_id: &EntityUri) -> Option<OperationIntent> {
        let contents = self.router.block_contents.lock().unwrap();
        for tree in contents.values() {
            if let Some(intent) = crate::focus_path::find_click_intent_oneshot(tree, entity_id) {
                return Some(intent);
            }
        }
        None
    }

    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        let contents = self.router.block_contents.lock().unwrap();
        for tree in contents.values() {
            let mut result: Option<String> = None;
            walk_tree(tree, &mut |n| {
                if result.is_some() {
                    return;
                }
                if n.entity_id().as_ref() == Some(entity_id) {
                    result = n.prop_str("content");
                }
            });
            if result.is_some() {
                return result;
            }
        }
        None
    }
}

/// Map a `Region` to the block id of the default-layout panel that hosts
/// it. The PBT seeds the default layout (`assets/default/index.org`); custom
/// layouts that move panels to other block ids would need this map updated
/// — but the PBT generators that drive `Region`-based queries are scoped to
/// the default layout for exactly that reason.
fn region_panel_block_id(region: holon_api::Region) -> EntityUri {
    let key = match region {
        holon_api::Region::LeftSidebar => "block:default-left-sidebar",
        holon_api::Region::Main => "block:default-main-panel",
        holon_api::Region::RightSidebar => "block:default-right-sidebar",
    };
    EntityUri::parse(key).expect("static panel-key literals are valid EntityUris")
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
    block_contents: Arc<Mutex<HashMap<EntityUri, Arc<ReactiveViewModel>>>>,
    /// block_id → drain task handle.
    watches: Arc<Mutex<HashMap<EntityUri, tokio::task::JoinHandle<()>>>>,
    /// Set when the first emission has been applied.
    ready: Arc<tokio::sync::Notify>,
    /// Root block id, established on the first `ensure_block_watch` call.
    root_block_id: OnceLock<EntityUri>,
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
        .and_then(|s| s.parse().ok()) // ALLOW(ok): malformed env var → use default
        .unwrap_or(DEFAULT_DROP_WIDGET_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn quiescence_config() -> (Duration, Duration) {
    let window = std::env::var("HOLON_PBT_QUIESCENCE_MS")
        .ok()
        .and_then(|s| s.parse().ok()) // ALLOW(ok): malformed env var → use default
        .unwrap_or(DEFAULT_QUIESCENCE_MS);
    let timeout = std::env::var("HOLON_PBT_QUIESCENCE_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok()) // ALLOW(ok): malformed env var → use default
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
            .map(|r| contents.contains_key(r))
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
                        .push((block_id.to_string(), row));
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

    fn bubble_input(&self, entity_id: &EntityUri, input: &WidgetInput) -> Option<InputAction> {
        let contents = self.block_contents.lock().unwrap();
        let root_id = self.root_block_id.get()?;
        let root_content = contents.get(root_id)?;
        let fp =
            crate::focus_path::build_focus_path_cross_block(root_content, &contents, entity_id)?;
        fp.bubble_input(entity_id, input)
    }

    fn ensure_block_watch(self: &Arc<Self>, block_id: &EntityUri) {
        let _ = self.root_block_id.set(block_id.clone());

        {
            let watches = self.watches.lock().unwrap();
            if watches.contains_key(block_id) {
                return;
            }
        }

        let stream = self.engine.watch(block_id);
        let router_weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        let cancelled = self.cancelled.clone();
        let bid = block_id.clone();

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
            .insert(block_id.clone(), handle);
    }

    fn process_emission(self: &Arc<Self>, block_id: &EntityUri, rvm: Arc<ReactiveViewModel>) {
        let was_first_root = {
            let mut contents = self.block_contents.lock().unwrap();
            let is_root = self
                .root_block_id
                .get()
                .map(|r| r == block_id)
                .unwrap_or(false);

            if contents.is_empty() && !is_root {
                tracing::debug!(
                    block_id = %block_id,
                    "process_emission: dropping pre-root nested emission"
                );
                return;
            }

            let was_empty = contents.is_empty();
            contents.insert(block_id.clone(), rvm.clone());
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
fn collect_nested_block_refs(node: &ReactiveViewModel, out: &mut HashSet<EntityUri>) {
    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            out.insert(
                EntityUri::parse(&block_id)
                    .expect("live_block props[\"block_id\"] must be a schemed EntityUri"),
            );
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
