//! [`DriverInputComponent`] — a driver-agnostic SUT input/focus provider.
//!
//! @pbt kind sut-component
//! @pbt gen NOT a generator — no input distribution lives here; every gesture
//!   (click/drag/arrow/slash/keystroke) forwards verbatim to the production
//!   `UserDriver`. Which ids, regions, keys, and step-counts are exercised is
//!   decided entirely by the block-interaction / arrow-navigate transition
//!   generators upstream. `resolver` remaps oracle ids to SUT-minted ids
//!   (headless composed only) so a gesture never drives a ghost block.
//!
//! Wraps any production [`UserDriver`] and provides the gesture caps
//! ([`SutDriver`] focus-read, plus [`SutBlockInteract`] + [`SutArrowNavigate`]
//! input when a driver is installed) by forwarding to it — it NEVER
//! re-implements input (honesty gate). It is **not** GPUI-specific: the
//! windowed builds wrap the live window's `GpuiUserDriver`/`SimUserDriver` + a
//! `GeometryProvider`; the headless composed build (`with_input_headless`)
//! wraps a `ReactiveEngineDriver` with no geometry (the UI-adjacent VM rung,
//! §8.11). The geometry is `Option`, so the type itself carries no window
//! dependency — hence this GPUI-independent home (moved off
//! `window_slice::components`, 2026-06-26).

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::navigation::NavDirection;
use holon_frontend::pbt_caps::SutArrowNavigate;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EngineFocus;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

use crate::pbt::op_write_cap::IdResolver;

/// The windowed slice's [`SutDriver`] provider — and, when built with a live
/// [`UserDriver`] (E4 `window_input_wide`), the windowed [`SutBlockInteract`] +
/// [`SutArrowNavigate`] **input** provider too.
///
/// The load-bearing read path stays: `inv-window-focus-matches-engine-focus`
/// binds `SutDriver::engine_focused_block`, which reads the **window's own**
/// frontend [`ReactiveEngine`] focus (the same engine
/// `GpuiFrontendEngineComponent` renders from), compared against the per-frame
/// window focus `SutLayout` reports.
///
/// E4 — real windowed input. When `driver`/`geometry` are present (the
/// `window_input_wide` build), this component WRAPS the production
/// [`UserDriver`] (the `SimUserDriver`/`GpuiUserDriver` the window installs) so
/// the composed `CapMap` itself can drive `PressKey` / `ArrowNavigate` / drag /
/// click — the same gestures `E2ESut`'s `SutBlockInteract` impl funnels into
/// its own `Arc<dyn UserDriver>`. Every driving method is a thin forward to
/// `self.driver`; the harness owns all window pumping (the methods are pure
/// `&self` reads/dispatches), so a windowed `CapMap` registering these caps
/// stops the input transitions from cap-gating out (the value-level cap gate in
/// `transition_dispatch::aggregate_transitions`). The methods **never
/// re-implement input** — that would be a vacuous self-test (honesty gate).
///
/// The focus-only `window_focus_wide` build leaves `driver`/`geometry` `None`:
/// it provides only `SutDriver` (to read focus), never the input caps, so its
/// `cap_set()` is unchanged.
///
/// `forced_engine_focus` is the **planted negative control**: when `Some`,
/// `engine_focused_block` returns it verbatim instead of the live engine focus,
/// letting a test inject an engine/window focus divergence (the steal-back /
/// zombie-editor fault, ADR 0010) to prove the invariant *bites* — the
/// focus-axis analogue of increment 3a's `Plant::Content` reference plant.
pub struct DriverInputComponent {
    engine: Arc<ReactiveEngine>,
    forced_engine_focus: Option<EngineFocus>,
    /// The production `UserDriver` of the live window. `Some` only for the
    /// `window_input_wide` build — its presence is what makes the component a
    /// `SutBlockInteract` + `SutArrowNavigate` provider (see `register`). Every
    /// input gesture forwards here; the component never synthesizes input
    /// itself.
    driver: Option<Arc<dyn UserDriver>>,
    /// The same window geometry `GpuiWindowComponent` reads. Used for the
    /// single-shot bounds precheck before a click/drag (mirror of
    /// `GpuiWindowComponent::wait_for_bounds`): the harness settles the frame
    /// to a fixed point before reads, so a poll loop would only spin.
    geometry: Option<Box<dyn GeometryProvider>>,
    /// True for the `with_input_headless` build (the VM-rung driver in the
    /// headless composed `CapMap`). It gates OFF the `SutDriver` cap in
    /// `register`: headless, `engine_focused_block` reads the frontend
    /// `ReactiveEngine`'s global focus, which is honestly `None` for a
    /// non-editor page block (no editor mounts without a window), so
    /// claiming `SutDriver` would select the **windowed** focus invariants
    /// (`inv-focus-matches-ref`, `inv-window-focus-matches-engine-focus`)
    /// over a focus signal they were never meant to read — a faked cap.
    /// Headless focus coherence is already covered by
    /// `inv-navigation-focus`/`inv-focus-roots` (the `SutFocusWrite`/
    /// `SutSqlProjection` path). So the headless build provides
    /// only the gesture caps (`SutBlockInteract`/`SutArrowNavigate`).
    headless: bool,
    /// Synthetic-oracle→SUT-real id map (`Some` only for the headless composed
    /// build, which drives an id-minting backend: a split mints a fresh
    /// uuid that the harness reconciles onto the oracle's
    /// `block::split-N`). Gesture methods resolve every incoming id through
    /// it before driving — without this a gesture targeting a minted block
    /// drives a ghost. `None` for the windowed builds, whose fixed shared
    /// ids need no remap (identity).
    resolver: Option<IdResolver>,
}

impl DriverInputComponent {
    /// Read the live window engine focus.
    pub fn new(engine: Arc<ReactiveEngine>) -> Self {
        Self {
            engine,
            forced_engine_focus: None,
            driver: None,
            geometry: None,
            headless: false,
            resolver: None,
        }
    }

    /// Force `engine_focused_block` to report `focus` regardless of the live
    /// engine — the planted divergence used to prove `inv-window-focus-matches-
    /// engine-focus` fails on a real engine/window mismatch.
    pub fn with_forced_engine_focus(engine: Arc<ReactiveEngine>, focus: EngineFocus) -> Self {
        Self {
            engine,
            forced_engine_focus: Some(focus),
            driver: None,
            geometry: None,
            headless: false,
            resolver: None,
        }
    }

    /// E4 — the full windowed input provider. Wraps the window's production
    /// `UserDriver` (`driver`) and reads its geometry (`geometry`) for the
    /// bounds precheck, so the composed `CapMap` can drive real windowed input
    /// (`SutBlockInteract` + `SutArrowNavigate`) on top of reading focus.
    pub fn with_input(
        engine: Arc<ReactiveEngine>,
        driver: Arc<dyn UserDriver>,
        geometry: Box<dyn GeometryProvider>,
    ) -> Self {
        Self {
            engine,
            forced_engine_focus: None,
            driver: Some(driver),
            geometry: Some(geometry),
            headless: false,
            resolver: None,
        }
    }

    /// The **headless** input provider: wraps a headless production
    /// `UserDriver` (`ReactiveEngineDriver`) with **no geometry**. This is
    /// the UI-adjacent logic layer for the headless composed `CapMap` —
    /// input gestures drive the same production logic the real UI runs
    /// (`click_entity` resolves the bound click intent via
    /// `find_click_intent_in_region`, `send_raw_keystroke` edits through
    /// the `MutableText`/`InputState` mirror), just without a
    /// platform window. Without geometry there is no bounds frame to gate on,
    /// so the single-shot bounds precheck is skipped (the driver resolves
    /// targets itself — intent lookup / shadow-tree walk); see
    /// [`Self::require_bounds`].
    pub fn with_input_headless(
        engine: Arc<ReactiveEngine>,
        driver: Arc<dyn UserDriver>,
        resolver: IdResolver,
    ) -> Self {
        Self {
            engine,
            forced_engine_focus: None,
            driver: Some(driver),
            geometry: None,
            headless: true,
            resolver: Some(resolver),
        }
    }

    /// The installed production driver. Panics (fail-loud) if an input gesture
    /// is invoked on a focus-only build — the build mistake, not silent
    /// no-op.
    fn driver(&self) -> &Arc<dyn UserDriver> {
        self.driver.as_ref().expect(
            "DriverInputComponent: windowed input gesture invoked without an installed UserDriver \
             — build with `window_input_wide` (with_input), not `window_focus_wide`",
        )
    }

    /// Resolve an oracle-space id to its SUT-space id (identity when no
    /// resolver is installed — the windowed fixed-id builds). The
    /// id-minting headless composed build shares the runner's `IdResolver`
    /// so a gesture targeting a minted block (e.g. `block::split-N`) drives
    /// the real block, not a ghost.
    fn resolve(&self, id: &EntityUri) -> EntityUri {
        match &self.resolver {
            Some(r) => r
                .lock()
                .expect("resolver lock")
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.clone()),
            None => id.clone(),
        }
    }

    /// Single-shot "are this id's bounds in the settled frame?" — same id-shape
    /// fan-out as [`GpuiWindowComponent::has_registered_bounds`].
    fn has_registered_bounds(&self, id: &EntityUri) -> bool {
        let geom = self.geometry.as_deref().expect(
            "DriverInputComponent: bounds precheck without installed geometry (use with_input)",
        );
        geom.element_info(&format!("render-entity-{id}"))
            .or_else(|| geom.element_info(&format!("live-block-{id}")))
            .or_else(|| geom.element_info(&format!("selectable-{id}")))
            .or_else(|| geom.element_info(&format!("editable-text-{id}")))
            .or_else(|| geom.find_by_entity_id(id.as_str()))
            .is_some()
    }

    /// Single-shot bounds gate (mirror of
    /// `GpuiWindowComponent::wait_for_bounds`): the harness pumps to a
    /// fixed point before driving, so this never spins.
    ///
    /// Headless (no geometry installed): there is no bounds frame to gate on —
    /// the headless driver resolves its target itself (`find_click_intent` /
    /// shadow-tree walk), so the precheck is skipped. Windowed: assert the id
    /// is present in the settled frame before driving real input at its
    /// coords.
    fn require_bounds(&self, id: &EntityUri, ctx: &str) {
        if self.geometry.is_none() {
            return;
        }
        assert!(
            self.has_registered_bounds(id),
            "[{ctx}] no registered bounds for {id} in the settled frame"
        );
    }

    /// Poll the window engine's focus until it lands on `id` or `timeout`
    /// elapses — the post-click barrier `SutDriver::wait_for_engine_focus`
    /// documents (GPUI's click dispatch is fire-and-forget).
    /// Runtime-agnostic awaits: the windowed per-tick path drives these via
    /// `futures::executor::block_on` against the harness's ambient runtime
    /// (same pattern as `resolve_watch`).
    async fn poll_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.engine.focused_block().as_ref() == Some(id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "engine focus never reached {id} within {timeout:?} (now: {:?})",
                    self.engine.focused_block()
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Headless editor-readiness gate: poll until the block's live
    /// `MutableText` content cell resolves, so a following single-char
    /// keystroke (e.g. the slash menu's `/`) has somewhere to land. The
    /// windowed path gates this via
    /// `SutLayout::wait_for_window_focused_editor` (real geometry frame);
    /// headless has no such frame, so a gesture targeting a freshly-created
    /// block (e.g. a just-split block whose Loro `content_raw` is still
    /// landing) would otherwise race the `HeadlessEditorMirror`. No-op
    /// windowed (geometry present → its own gate).
    async fn ensure_editor_ready(
        &self,
        id: &EntityUri,
        timeout: Duration,
        ctx: &str,
    ) -> Result<(), String> {
        if self.geometry.is_some() {
            return Ok(());
        }
        let services: &dyn holon_frontend::reactive::BuilderServices = self.engine.as_ref();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if services.editable_text(id, "content").is_ok() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "[{ctx}] editor for {id} never became ready (no MutableText / Loro \
                     content_raw unresolved) within {timeout:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutDriver for DriverInputComponent {
    /// Not wired here for the same reason `E2ESut` leaves it `unimplemented!`:
    /// the chord verbs route through `send_key_chord` with an explicit focused
    /// entity id + parsed `KeyChord`; no transition binds this method.
    async fn driver_send_key_chord(&self, _: &str) {
        unimplemented!(
            "DriverInputComponent::driver_send_key_chord: requires a focused entity id and a \
             parsed KeyChord; transitions drive clicks/keystrokes via the other SutDriver verbs"
        )
    }

    /// Region-defaulted click convenience — forwards to `click_entity` (region
    /// "main"), panicking on driver error (mirror of `E2ESut::driver_click`).
    async fn driver_click(&self, id: &EntityUri) {
        <Self as SutDriver>::click_entity(self, id, "main")
            .await
            .unwrap_or_else(|e| panic!("SutDriver::driver_click failed for {id}: {e}"));
    }

    /// Region-aware click — a thin forward to the production `UserDriver`.
    async fn click_entity(&self, id: &EntityUri, region: &str) -> Result<(), String> {
        self.driver()
            .click_entity(id, region)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Post-click focus barrier — polls the window engine's `focused_block`.
    async fn wait_for_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String> {
        self.poll_engine_focus(id, timeout).await
    }

    /// Single raw keystroke — a thin forward to the production `UserDriver`.
    async fn send_raw_keystroke(&self, key: &str, modifiers: &[&str]) -> Result<(), String> {
        self.driver()
            .send_raw_keystroke(key, modifiers)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    async fn driver_current_focus(&self) -> Option<EntityUri> {
        self.engine.focused_block()
    }

    /// The load-bearing method: the window engine's globally focused block,
    /// mapped into [`EngineFocus`]. `forced_engine_focus` overrides it for
    /// the planted negative control.
    async fn engine_focused_block(&self) -> EngineFocus {
        if let Some(forced) = &self.forced_engine_focus {
            return forced.clone();
        }
        match self.engine.focused_block() {
            None => EngineFocus::Unfocused,
            Some(id) => EngineFocus::Focused(id),
        }
    }

    /// The windowed slice uses fixed shared ids (no synthetic ref-doc URIs to
    /// remap), so the id passes through unchanged.
    fn resolve_ref_block_id(&self, id: &EntityUri) -> EntityUri {
        id.clone()
    }
}

/// E4 — windowed block-level input. Every method WRAPS the production
/// [`UserDriver`] (`self.driver`), mirroring `E2ESut`'s `SutBlockInteract`
/// bodies (`sut_handle.rs`) minus the `RefCell` (this component owns the `Arc`)
/// and the `ref_state`-dependent post-actions (the harness reconcile/settle
/// seam owns those, per the `ref_state`-off-the-cap principle). The bounds
/// precheck reads `self.geometry` single-shot; root resolution uses the layout
/// root the engine renders from.
#[async_trait::async_trait(?Send)]
impl SutBlockInteract for DriverInputComponent {
    async fn click_block(&self, region: holon_api::Region, block_id: &EntityUri) {
        let resolved = self.resolve(block_id);
        let block_id = &resolved;
        self.require_bounds(block_id, "ClickBlock");
        self.driver()
            .click_entity(block_id, region.as_str())
            .await
            .unwrap_or_else(|e| panic!("[ClickBlock] click_entity failed for {block_id}: {e:#}"));
        self.poll_engine_focus(block_id, Duration::from_secs(2))
            .await
            .unwrap_or_else(|e| panic!("[ClickBlock] focus did not propagate for {block_id}: {e}"));
        // Let CDC propagate (mirrors the yield_now dance the E2ESut body runs).
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    async fn drag_drop_block(&self, source: &EntityUri, target: &EntityUri) {
        let (rs, rt) = (self.resolve(source), self.resolve(target));
        let (source, target) = (&rs, &rt);
        // Single-shot bounds gate against the already-settled frame — the windowed
        // `drop_entity` reads source+target geometry to synthesize the drag.
        self.require_bounds(source, "DragDropBlock(source)");
        self.require_bounds(target, "DragDropBlock(target)");
        // The screen `UserDriver` ignores the root arg (it drags by geometry); the
        // layout root is the faithful value a headless driver would warm.
        let root = holon_api::root_layout_block_uri();
        let dispatched = self
            .driver()
            .drop_entity(&root, source, target)
            .await
            .unwrap_or_else(|e| panic!("[DragDropBlock] drop_entity failed: {e:#}"));
        assert!(
            dispatched,
            "[DragDropBlock] drop_entity returned false for {source} → {target}"
        );
    }

    async fn expand_toggle(&self, block_id: &EntityUri) {
        let resolved = self.resolve(block_id);
        let block_id = &resolved;
        // Real chevron flip: `UserDriver::set_block_expanded` synthesizes a
        // click on the chevron registered under `expand_toggle_id_for(block_id)`,
        // so the production `on_mouse_down` handler flips the row's view-local
        // `expanded` Mutable — the same path a user's tap takes. (Previously
        // `unimplemented!` because no UserDriver verb wrapped this view-local
        // gesture.)
        self.driver()
            .set_block_expanded(block_id, true)
            .await
            .unwrap_or_else(|e| {
                panic!("[ExpandToggle] set_block_expanded({block_id}, true) failed: {e:#}")
            });
    }

    async fn scroll_over(&self, element_id: &str, delta_y: f32) {
        // Windowed wheel: forward to the production `UserDriver`, which
        // synthesizes a real scroll-wheel event at the element's centre. The
        // element id may be a block URI (`block:default-main-panel`) or a raw
        // geometry handle (the sticky-footer id); parse leniently.
        let uri = holon_api::EntityUri::parse(element_id)
            .unwrap_or_else(|_| holon_api::EntityUri::block(element_id));
        self.driver()
            .scroll_entity(&uri, 0.0, delta_y)
            .await
            .unwrap_or_else(|e| panic!("[WheelScroll] scroll_entity({element_id}) failed: {e:#}"));
    }

    async fn collapse_toggle(&self, block_id: &EntityUri) {
        let resolved = self.resolve(block_id);
        let block_id = &resolved;
        self.driver()
            .set_block_expanded(block_id, false)
            .await
            .unwrap_or_else(|e| {
                panic!("[CollapseToggle] set_block_expanded({block_id}, false) failed: {e:#}")
            });
    }

    async fn trigger_slash_command(&self, block_id: &EntityUri) {
        let resolved = self.resolve(block_id);
        let block_id = &resolved;
        // Faithful port of `apply_trigger_slash_command_to_sut` driving the real
        // window: focus the editor, open the slash menu, filter to "delete",
        // press Enter — every step a production `UserDriver` gesture.
        self.require_bounds(block_id, "TriggerSlashCommand");
        self.driver()
            .click_entity(block_id, "main")
            .await
            .unwrap_or_else(|e| {
                panic!("[TriggerSlashCommand] click_entity failed for {block_id}: {e:#}")
            });
        self.poll_engine_focus(block_id, Duration::from_secs(1))
            .await
            .unwrap_or_else(|e| {
                panic!("[TriggerSlashCommand] focus did not propagate for {block_id}: {e}")
            });
        // Headless: wait for the focused block's editor cell before the single-char
        // `/` — a freshly-created block (e.g. a just-split target) may still be landing
        // its Loro `content_raw`, which the `HeadlessEditorMirror` needs for `/`.
        self.ensure_editor_ready(block_id, Duration::from_secs(2), "TriggerSlashCommand")
            .await
            .unwrap_or_else(|e| panic!("[TriggerSlashCommand] {e}"));
        let driver = self.driver();
        driver
            .send_raw_keystroke("/", &[])
            .await
            .unwrap_or_else(|e| panic!("[TriggerSlashCommand] '/' keystroke failed: {e:#}"));
        for ch in "delete".chars() {
            driver
                .send_raw_keystroke(&ch.to_string(), &[])
                .await
                .unwrap_or_else(|e| {
                    panic!("[TriggerSlashCommand] filter char {ch:?} keystroke failed: {e:#}")
                });
        }
        driver
            .send_raw_keystroke("enter", &[])
            .await
            .unwrap_or_else(|e| panic!("[TriggerSlashCommand] Enter keystroke failed: {e:#}"));
    }

    async fn press_key(&self, chord: &holon_api::KeyChord) {
        // Faithful port of `E2ESut::SutBlockInteract::press_key`: split the chord
        // into modifiers + regular keys and forward each as a raw keystroke. The
        // `ref_state`-dependent Enter-split post-action lives in the harness seam.
        use holon_api::Key;
        let driver = self.driver();
        let modifiers: Vec<String> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Cmd => Some("cmd".to_string()),
                Key::Ctrl => Some("ctrl".to_string()),
                Key::Alt => Some("alt".to_string()),
                Key::Shift => Some("shift".to_string()),
                _ => None,
            })
            .collect();
        let regulars: Vec<&'static str> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Enter => Some("enter"),
                Key::Backspace => Some("backspace"),
                Key::Tab => Some("tab"),
                Key::Escape => Some("escape"),
                _ => None,
            })
            .collect();
        let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
        for key in regulars {
            driver
                .send_raw_keystroke(key, &mod_refs)
                .await
                .unwrap_or_else(|e| panic!("[PressKey] send_raw_keystroke({key}) failed: {e:#}"));
        }
    }

    async fn click_at_element(&self, element_id: &str) {
        // Faithful port of `E2ESut::click_at_element`: infer the region from the
        // id prefix, unwrap a `<kind>::<uri>` geometry handle to its target block
        // uri, and click through the production driver.
        let region = if element_id.contains("left-sidebar") || element_id.contains("left_sidebar") {
            "left_sidebar"
        } else if element_id.contains("right-sidebar") || element_id.contains("right_sidebar") {
            "right_sidebar"
        } else {
            "main"
        };
        let target = match element_id.split_once("::") {
            Some((kind, suffix)) if !kind.contains(':') && suffix.contains(':') => suffix,
            _ => element_id,
        };
        let element_uri = EntityUri::parse(target).unwrap_or_else(|e| {
            panic!("[click_at_element] {element_id:?} (target {target:?}) is not an EntityUri: {e}")
        });
        self.driver()
            .click_entity(&element_uri, region)
            .await
            .unwrap_or_else(|e| {
                panic!("[click_at_element] click_entity({element_id}) failed: {e:#}")
            });
    }
}

/// E4 — windowed arrow-key navigation. Faithful port of `E2ESut`'s
/// [`SutArrowNavigate`] impl (`sut_handle.rs`): map the direction to a raw
/// keystroke and send it `steps` times through the production driver's
/// retry-until-handled path (covers the editor-mount race after a focus move).
#[async_trait::async_trait(?Send)]
impl SutArrowNavigate for DriverInputComponent {
    async fn apply_arrow_navigate(&self, _: CapRegion, direction: NavDirection, steps: u8) {
        use holon_frontend::navigation::NavDirection::Down;
        use holon_frontend::navigation::NavDirection::Left;
        use holon_frontend::navigation::NavDirection::Right;
        use holon_frontend::navigation::NavDirection::Up;
        let keystroke = match direction {
            Up => "up",
            Down => "down",
            Left => "left",
            Right => "right",
        };
        let driver = self.driver();
        for _ in 0..steps {
            driver
                .send_raw_keystroke_until_handled(keystroke, &[], Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ArrowNavigate] keystroke '{keystroke}' failed: {e:#}")
                });
        }
    }
}

impl CapProvider for DriverInputComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        // `SutDriver` (the window-focus read) is a WINDOWED cap: its consumers are
        // the windowed focus invariants (`inv-focus-matches-ref`,
        // `inv-window-focus-matches-engine-focus`). The headless VM-rung build does
        // NOT provide it — `engine_focused_block` is honestly `None` for a non-editor
        // page block headless, so claiming the cap would select those windowed
        // invariants over a focus signal they were never meant to read (faked cap).
        if !self.headless {
            caps.insert(self.clone() as Arc<dyn SutDriver>);
        }
        // The input caps are provided ONLY when a real `UserDriver` is installed
        // (the `window_input_wide` build, or the headless VM-rung build). A
        // focus-only `window_focus_wide` component reads focus but cannot honestly
        // drive, so it must not claim these caps — keeping its `cap_set()` (and the
        // transition alphabet it would gate) unchanged.
        if self.driver.is_some() {
            caps.insert(self.clone() as Arc<dyn SutBlockInteract>);
            caps.insert(self as Arc<dyn SutArrowNavigate>);
        }
    }
}
