//! TestPlatform-backed windowed replay service — synchronous, single-threaded,
//! no channels, no real NSWindow.
//!
//! Replaces the channel-based `WindowHost`/`WindowedReplayer` pair from
//! `windowed_replay.rs` with a direct-dispatch variant that runs inside
//! `TestApp`. The proptest state machine, signature pinning, capture format,
//! and `seen_counter` semantics are unchanged — this swaps only the host.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, InputEvent, Keystroke, MouseButton, Pixels, Point, TestApp};
use holon_api::{EntityUri, KeyChord, Region, Value};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_frontend::{OperationIntent, ReactiveViewModel};
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::RebindHandle;
use holon_integration_tests::pbt::composed::composed_invariant_catalog;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::phased::{
    replay_fixture_with_driver_sync_callback, PbtReadyContext, PbtReadyResult,
};
use holon_integration_tests::pbt::reference_capabilities::reference_state_ref_caps;
use holon_integration_tests::pbt::reference_state::Resolved;
use holon_integration_tests::pbt::window_slice::builders::window_focus_wide;
use holon_integration_tests::pbt::ReferenceState;
use holon_pbt_core::composition::run_selected;

/// Wait on tokio (real time) until the engine's root layout signal produces
/// a non-loading view model. Under TestPlatform gpui timers time-skip, so the
/// in-launch pre-warm (`launch_holon_window_impl`) always "times out" after
/// microseconds of real time — warm the watcher here instead, before the
/// window subscribes; the signal then replays the warm value on subscription.
pub(crate) fn warm_root_signal(engine: &Arc<ReactiveEngine>, rt_handle: &tokio::runtime::Handle) {
    use futures::StreamExt;
    use futures_signals::signal::SignalExt;
    let root_uri = holon_api::root_layout_block_uri();
    let sig = engine.watch_data_signal(&root_uri);
    let warmed = rt_handle.block_on(async move {
        let mut stream = sig.to_stream();
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(rvm) = stream.next().await {
                if rvm.widget_name().as_deref() != Some("loading") {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    });
    assert!(
        warmed,
        "root layout signal never produced real data within 30s (engine watcher stalled)"
    );
}

/// One sim pump cycle: real tokio time for backend watchers, drain gpui
/// tasks (test builds draw dirty windows inside flush_effects), fire fake
/// timers, drain again, promote staged bounds.
fn pump_cycle(app: &TestApp, bounds: &BoundsRegistry) {
    // Real wall-clock pause: the backend's multi-thread tokio runtime
    // advances on its own worker threads — no block_on needed (and driver
    // methods may already be inside a tokio context where block_on panics).
    std::thread::sleep(Duration::from_millis(10));
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(500));
    app.run_until_parked();
    bounds.flush();
}

/// Settle to a fixed point: pump until the BoundsRegistry element count is
/// stable across consecutive cycles and no "loading" placeholders remain.
/// Bounded — panics loudly if the tree never stabilizes.
fn settle_to_fixed_point(app: &TestApp, bounds: &BoundsRegistry, max_cycles: usize) {
    let mut last_count = usize::MAX;
    let mut stable = 0u32;
    for _ in 0..max_cycles {
        pump_cycle(app, bounds);
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count > 0 && count == last_count && !still_loading {
            stable += 1;
            if stable >= 3 {
                return;
            }
        } else {
            stable = 0;
        }
        last_count = count;
    }
    let elements = bounds.all_elements();
    panic!(
        "sim settle never reached a fixed point after {max_cycles} cycles: \
         {} elements, loading={}",
        elements.len(),
        elements
            .iter()
            .filter(|(_, info)| info.widget_type.as_ref() == "loading")
            .count()
    );
}

// ---------------------------------------------------------------------------
// SimUserDriver — UserDriver over direct TestPlatform dispatch
// ---------------------------------------------------------------------------

/// Direct-dispatch UserDriver for TestPlatform. Holds a raw pointer to the
/// `TestApp` owned by `SimReplayer`. SAFETY: SimReplayer outlives all
/// SimUserDriver instances; all access is from one thread.
pub(crate) struct SimUserDriver {
    app_ptr: *const TestApp,
    window: gpui::AnyWindowHandle,
    bounds: BoundsRegistry,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    /// Interaction-pump channel (set on the shared `DebugServices` by the
    /// window's `setup_interaction_pump`). Used for `ScrollEntityIntoView`,
    /// which must walk the entity cache to reveal virtualized-list rows —
    /// not something the driver can do via raw scroll events.
    interaction_tx: futures::channel::mpsc::Sender<holon_mcp::server::InteractionCommand>,
}

unsafe impl Sync for SimUserDriver {}
unsafe impl Send for SimUserDriver {}

impl SimUserDriver {
    /// Construct a driver over an already-launched, settled window. SAFETY: the
    /// caller must keep the `TestApp` `app_ptr` points at alive and untouched for
    /// the driver's lifetime, and use the driver only from the gpui thread (same
    /// contract `SimReplayer::replay` relies on). Used by the windowed composition
    /// slice (`gpui_window_slice`) to drive a single focus interaction before
    /// reading `inv-window-focus-matches-engine-focus` over the composed `CapMap`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app_ptr: *const TestApp,
        window: gpui::AnyWindowHandle,
        bounds: BoundsRegistry,
        engine: Arc<ReactiveEngine>,
        rt_handle: tokio::runtime::Handle,
        interaction_tx: futures::channel::mpsc::Sender<holon_mcp::server::InteractionCommand>,
    ) -> Self {
        Self {
            app_ptr,
            window,
            bounds,
            engine,
            rt_handle,
            interaction_tx,
        }
    }

    /// Dispatch + cross-runtime settle. Every interaction that modifies
    /// state MUST settle before the next one: advance tokio so backend
    /// signals deliver → drain gpui tasks → promote staged bounds.
    fn update_and_settle<R>(&self, f: impl FnOnce(&mut gpui::App) -> R) -> R {
        let app = unsafe { &mut *(self.app_ptr as *mut TestApp) };
        let r = app.update(|cx| f(cx));
        // Release &mut, then advance tokio + pump gpui + flush bounds.
        let app = unsafe { &*self.app_ptr };
        pump_cycle(app, &self.bounds);
        r
    }

    /// Pump gpui without sleeping. In sim mode nothing else drives the gpui
    /// loop while the harness polls driver reads, so every read must drain
    /// pending tasks (test builds draw dirty windows in flush_effects) and
    /// promote staged bounds — otherwise polls spin on a frozen frame.
    fn pump(&self) {
        let app = unsafe { &*self.app_ptr };
        app.run_until_parked();
        self.bounds.flush();
    }

    /// Mirror `GpuiUserDriver::element_center`: block rows register under
    /// `render-entity-{id}` / `selectable-{id}` aliases (sidebar rows have no
    /// outer `render_entity()`), non-block UI handles under their raw id.
    /// Visible-area gate keeps clipped overdraw rows from resolving to a
    /// center on top of a different row.
    fn bounds_center_f32(&self, entity_id: &EntityUri) -> Option<(f32, f32)> {
        self.pump();
        let entity_id = entity_id.as_str();
        for el_id in [
            format!("render-entity-{entity_id}"),
            format!("selectable-{entity_id}"),
            entity_id.to_string(),
        ] {
            if let Some(info) = self.bounds.element_info(&el_id) {
                if info.has_visible_area() {
                    return Some(info.center());
                }
            }
        }
        self.bounds
            .find_by_entity_id_visible(entity_id)
            .map(|info| info.center())
    }

    fn mouse_point(&self, entity_id: &EntityUri) -> Option<Point<Pixels>> {
        let (cx, cy) = self.bounds_center_f32(entity_id)?;
        Some(Point {
            x: Pixels::from(cx),
            y: Pixels::from(cy),
        })
    }

    /// Dispatch one keystroke; returns gpui's `handled` flag.
    fn dispatch_keystroke_once(&self, ks: &Keystroke) -> bool {
        self.update_and_settle(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_keystroke(ks.clone(), cx)
                })
                .unwrap()
        })
    }

    /// Mirror `GpuiUserDriver::key_down_until_handled`: retry — pumping a
    /// full cycle between attempts — until a handler consumes the key.
    /// Covers the editor-mount race (engine focus moved, editor takes
    /// window focus on a later render pass). Unconsumed keystrokes have no
    /// side effects, so the retry cannot double-apply. Fails loud at the
    /// deadline: a key the UI never consumes is a keybinding/focus bug.
    fn dispatch_keystroke_until_handled(
        &self,
        ks_str: &str,
        timeout: Duration,
        context: &str,
    ) -> Result<(), anyhow::Error> {
        let ks = Keystroke::parse(ks_str)
            .map_err(|e| anyhow::anyhow!("{context}: bad keystroke {ks_str:?}: {e}"))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.dispatch_keystroke_once(&ks) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("{context}: keystroke {ks_str:?} never consumed within {timeout:?}");
            }
        }
    }

    /// Raw click at `pos` (no settle decisions — callers pump).
    fn raw_click(&self, pos: Point<Pixels>) {
        self.update_and_settle(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::MouseDownEvent {
                            position: pos,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                            first_mouse: false,
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseUpEvent {
                            position: pos,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
    }
}

#[async_trait::async_trait]
impl UserDriver for SimUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<(), anyhow::Error> {
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    /// Mirror `GpuiUserDriver::send_key_chord`: click-to-focus (unless the
    /// target already holds engine focus), wait for the target's
    /// editable_text to take WINDOW focus in a committed frame, optionally
    /// seed the caret (`home` + `right`×N), then press the chord keys —
    /// each retried until consumed.
    async fn send_key_chord(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool, anyhow::Error> {
        use holon_gpui::user_driver::{is_modifier, keystroke_name, modifier_name};
        let id = entity_id.as_str();

        // Click-to-focus with re-click until focused_block lands on the
        // target — a single click can land on a neighbor row when an async
        // commit shifts bounds mid-click.
        let overall_deadline = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            let already_focused = self
                .engine
                .ui_state()
                .focused_block_mutable()
                .get_cloned()
                .as_ref()
                .map(|u| u.as_str())
                == Some(id);
            if already_focused {
                break;
            }
            let Some(pos) = self.mouse_point(entity_id) else {
                anyhow::bail!(
                    "send_key_chord: entity {entity_id} not in bounds for click-to-focus"
                );
            };
            self.raw_click(pos);
            let attempt_deadline = std::time::Instant::now() + Duration::from_secs(1);
            let landed = loop {
                let focused = self
                    .engine
                    .ui_state()
                    .focused_block_mutable()
                    .get_cloned()
                    .as_ref()
                    .map(|u| u.as_str())
                    == Some(id);
                if focused {
                    break true;
                }
                if std::time::Instant::now() >= attempt_deadline {
                    break false;
                }
                let app = unsafe { &*self.app_ptr };
                pump_cycle(app, &self.bounds);
            };
            if landed {
                break;
            }
            if std::time::Instant::now() >= overall_deadline {
                anyhow::bail!(
                    "send_key_chord: click on {entity_id} never moved focused_block to it \
                     within 4s (incl. re-click attempts) — refusing to press {chord:?} into \
                     the wrong editor"
                );
            }
            eprintln!(
                "[send_key_chord] click did not land focus on {entity_id} (re-render likely \
                 shifted bounds mid-click); re-clicking"
            );
        }

        // Engine focus moved, but the editor takes WINDOW focus only on a
        // following render pass — wait for the target's editable_text to
        // report focused == true in a committed frame.
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                self.pump();
                let window_focused = self.bounds.all_elements().iter().any(|(_, info)| {
                    info.entity_id.as_deref() == Some(id)
                        && info.widget_type.as_ref() == "editable_text"
                        && info.focused == Some(true)
                });
                if window_focused {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "send_key_chord: {entity_id}'s editable_text never took window focus \
                         within 2s of click-to-focus — refusing to press {chord:?} into \
                         whatever editor still holds it"
                    );
                }
                let app = unsafe { &*self.app_ptr };
                pump_cycle(app, &self.bounds);
            }
        }

        // Caret seed: click lands the cursor somewhere in the line; re-anchor
        // with `home`, advance with `right` — through the real input path.
        if let Some(Value::Integer(target)) = extra_params.get("position") {
            let target = (*target).max(0) as usize;
            self.dispatch_keystroke_until_handled(
                "home",
                Duration::from_secs(2),
                "send_key_chord(position)",
            )?;
            for _ in 0..target {
                self.dispatch_keystroke_until_handled(
                    "right",
                    Duration::from_secs(2),
                    "send_key_chord(position)",
                )?;
            }
        }

        let (modifiers, regulars): (Vec<_>, Vec<_>) =
            chord.0.iter().cloned().partition(is_modifier);
        let mod_prefix: String = modifiers
            .iter()
            .filter_map(|k| modifier_name(k))
            .map(|m| format!("{m}-"))
            .collect();
        for key in &regulars {
            let Some(name) = keystroke_name(key) else {
                continue;
            };
            let ks_str = format!("{mod_prefix}{name}");
            self.dispatch_keystroke_until_handled(
                &ks_str,
                Duration::from_secs(2),
                "send_key_chord",
            )?;
        }
        Ok(true)
    }

    async fn click_entity(&self, entity_id: &EntityUri, _: &str) -> Result<(), anyhow::Error> {
        let Some(pos) = self.mouse_point(entity_id) else {
            anyhow::bail!("entity {entity_id} not in bounds");
        };
        self.raw_click(pos);
        Ok(())
    }

    async fn click_entity_with_tree(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        _: &str,
    ) -> Result<bool, anyhow::Error> {
        self.click_entity(entity_id, "").await?;
        Ok(false)
    }

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        self.pump();
        self.bounds
            .find_by_entity_id(entity_id.as_str())
            .filter(|info| info.area() > 0.0)
            .is_some()
    }

    fn is_in_region(&self, entity_id: &EntityUri, _: Region) -> bool {
        self.pump();
        self.bounds.find_by_entity_id(entity_id.as_str()).is_some()
    }

    fn entities_in_region(&self, _: Region) -> Vec<EntityUri> {
        self.pump();
        self.bounds
            .all_elements()
            .into_iter()
            .filter_map(|(_, info)| info.entity_id.and_then(|id| EntityUri::parse(&id).ok()))
            .collect()
    }

    fn reachable_entities_in_region(&self, region: Region) -> Vec<EntityUri> {
        self.entities_in_region(region)
    }

    /// Mirror `GpuiUserDriver::scroll_to_entity`: rows in a virtualized
    /// `gpui::list` outside the viewport are unmounted — only the
    /// `ScrollEntityIntoView` interaction (which walks the entity cache and
    /// calls `scroll_to_reveal_raw_index`) can reveal them. The pump task
    /// runs on gpui's foreground executor, so await the response by pumping;
    /// a plain `.await` would deadlock (nothing else drives gpui in sim).
    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<(), anyhow::Error> {
        let (resp_tx, mut resp_rx) = tokio::sync::oneshot::channel();
        self.interaction_tx
            .clone()
            .try_send(holon_mcp::server::InteractionCommand {
                event: holon_mcp::server::InteractionEvent::ScrollEntityIntoView {
                    entity_id: entity_id.to_string(),
                },
                response_tx: resp_tx,
            })
            .map_err(|e| anyhow::anyhow!("interaction channel send failed: {e}"))?;
        for _ in 0..200 {
            self.pump();
            match resp_rx.try_recv() {
                Ok(_) => {
                    // Let the reveal's scroll → mount → paint cascade commit.
                    let app = unsafe { &*self.app_ptr };
                    pump_cycle(app, &self.bounds);
                    return Ok(());
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    anyhow::bail!("interaction pump dropped the ScrollEntityIntoView response");
                }
            }
        }
        anyhow::bail!("ScrollEntityIntoView response never arrived after 200 pump cycles");
    }

    fn click_intent_of(&self, _: &EntityUri) -> Option<OperationIntent> {
        None
    }

    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        self.pump();
        self.bounds
            .find_by_entity_id(entity_id.as_str())
            .and_then(|info| info.displayed_text.map(|s| s.to_string()))
    }

    async fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) -> Result<(), anyhow::Error> {
        let point = Point {
            x: Pixels::from(x),
            y: Pixels::from(y),
        };
        self.update_and_settle(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::ScrollWheelEvent {
                            position: point,
                            delta: gpui::ScrollDelta::Lines(Point { x: dx, y: dy }),
                            modifiers: Default::default(),
                            touch_phase: gpui::TouchPhase::Moved,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
        Ok(())
    }

    async fn scroll_entity(
        &self,
        entity_id: &EntityUri,
        dx: f32,
        dy: f32,
    ) -> Result<(), anyhow::Error> {
        let Some((cx, cy)) = self.bounds_center_f32(entity_id) else {
            return Ok(());
        };
        self.scroll_at(cx, cy, dx, dy).await
    }

    async fn drop_entity(
        &self,
        _: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
    ) -> Result<bool, anyhow::Error> {
        let Some(src) = self.mouse_point(source_id) else {
            anyhow::bail!("source {source_id} not in bounds");
        };
        let Some(dst) = self.mouse_point(target_id) else {
            anyhow::bail!("target {target_id} not in bounds");
        };
        self.update_and_settle(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::MouseDownEvent {
                            position: src,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                            first_mouse: false,
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseMoveEvent {
                            position: dst,
                            modifiers: Default::default(),
                            pressed_button: Some(MouseButton::Left),
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseUpEvent {
                            position: dst,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
        Ok(true)
    }

    /// Mirror `GpuiUserDriver::send_raw_keystroke`: an unconsumed keystroke
    /// fails loud — masking it would ship stale state through invariants.
    async fn send_raw_keystroke(
        &self,
        keystroke: &str,
        modifiers: &[&str],
    ) -> Result<(), anyhow::Error> {
        let ks_str = modifiers
            .iter()
            .map(|m| format!("{m}-"))
            .collect::<String>()
            + keystroke;
        let ks = Keystroke::parse(&ks_str)
            .map_err(|e| anyhow::anyhow!("send_raw_keystroke: bad keystroke {ks_str:?}: {e}"))?;
        if !self.dispatch_keystroke_once(&ks) {
            anyhow::bail!(
                "GPUI keystroke not consumed: keystroke={keystroke:?} modifiers={modifiers:?}"
            );
        }
        // Pace like the real driver (one committed frame per keystroke):
        // give the backend echo cycle real tokio time to settle before the
        // next key, else sub-frame typing hits the focus-gated editor-echo
        // clobber race and drops leading characters ("ktFB" → "FB").
        let app = unsafe { &*self.app_ptr };
        pump_cycle(app, &self.bounds);
        pump_cycle(app, &self.bounds);
        Ok(())
    }

    async fn send_raw_keystroke_until_handled(
        &self,
        keystroke: &str,
        modifiers: &[&str],
        timeout: Duration,
    ) -> Result<(), anyhow::Error> {
        let ks_str = modifiers
            .iter()
            .map(|m| format!("{m}-"))
            .collect::<String>()
            + keystroke;
        self.dispatch_keystroke_until_handled(&ks_str, timeout, "send_raw_keystroke_until_handled")
    }

    /// `false`, mirroring `GpuiUserDriver` (which inherits the default):
    /// gpui has no leader-mode tracker, so a raw `SPC`+`h` sequence can
    /// never resolve to `go_home` — `send_leader_chord` must dispatch the
    /// navigation op synthetically, same as the real-window gpui PBT.
    fn dispatches_chords_via_raw_keystroke(&self) -> bool {
        false
    }

    fn editor_cursor_byte(&self, _: &EntityUri) -> Result<Option<usize>, String> {
        Err("editor caret not observable by SimUserDriver".to_string())
    }
}

// ---------------------------------------------------------------------------
// SimReplayer — synchronous replay host
// ---------------------------------------------------------------------------

pub(crate) struct SimReplayer {
    app: UnsafeCell<TestApp>,
    rebind_handle: UnsafeCell<RebindHandle>,
    window: gpui::AnyWindowHandle,
    bounds: BoundsRegistry,
    rt_handle: tokio::runtime::Handle,
    /// Shared `DebugServices` whose `interaction_tx` the window's
    /// interaction pump populated at launch (one per process, like the
    /// real-window host's single shared `DebugServices`).
    debug: Arc<holon_mcp::server::DebugServices>,
}

unsafe impl Sync for SimReplayer {}

impl SimReplayer {
    pub(crate) fn new(
        app: TestApp,
        rebind_handle: RebindHandle,
        bounds: BoundsRegistry,
        rt_handle: tokio::runtime::Handle,
        debug: Arc<holon_mcp::server::DebugServices>,
    ) -> Self {
        let window = rebind_handle.window();
        Self {
            app: UnsafeCell::new(app),
            rebind_handle: UnsafeCell::new(rebind_handle),
            window,
            bounds,
            rt_handle,
            debug,
        }
    }

    /// Shut down the app, then leak it. `App::shutdown` clears windows, but
    /// detached pump tasks (root-layout signal, interaction pump) and the
    /// RebindHandle's entity cache still hold `AppModel`/`ReactiveShell`
    /// handles, and gpui's leak detector runs before the dispatcher drops
    /// those tasks — so dropping `TestApp` always panics "leaked handles".
    /// The process exits right after; leaking here is disclosed and benign.
    pub(crate) fn dispose(self) {
        {
            let app = unsafe { &mut *self.app.get() };
            app.update(|cx| cx.shutdown());
            app.run_until_parked();
        }
        std::mem::forget(self);
    }

    pub fn replay(
        &self,
        wiring: holon_pbt_core::Wiring,
        steps: Vec<FixtureStep>,
        seen_counter: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Result<(), Box<dyn std::any::Any + Send>> {
        let app_ptr = self.app.get();
        let rebind_ptr = self.rebind_handle.get();
        let window = self.window;
        let bounds = self.bounds.clone();
        let rt_handle = self.rt_handle.clone();
        let interaction_tx = self
            .debug
            .interaction_tx
            .get()
            .expect("interaction_tx not set by window pump")
            .clone();

        // Engine slot shared between `on_ready` (which sets it once the frontend engine
        // exists at StartApp) and the per-tick hook (which reads it to build the windowed
        // `CapMap` each tick). Single-threaded gpui replay, but use Arc<Mutex> so the
        // closures carry no `!Send`/`!Sync` surprises.
        let engine_slot: Arc<std::sync::Mutex<Option<Arc<ReactiveEngine>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let engine_slot_ready = engine_slot.clone();

        let on_ready = move |ctx: &PbtReadyContext| -> Option<PbtReadyResult> {
            let app = unsafe { &mut *app_ptr };
            let rebind = unsafe { &*rebind_ptr };

            // Warm the new engine's root watcher in real time BEFORE the
            // window subscribes — under TestPlatform the launch pre-warm
            // time-skips and always misses the (real-time) tokio query.
            warm_root_signal(&ctx.reactive_engine, &rt_handle);

            // Rebind the window to the new engine.
            app.update(|cx| {
                rebind.rebind(ctx.session.clone(), ctx.reactive_engine.clone(), cx);
            });

            // Post-rebind settle: pump both runtimes until the rendered
            // tree reaches a fixed point (no loading placeholders, stable
            // element count).
            settle_to_fixed_point(unsafe { &*app_ptr }, &bounds, 500);

            // Hand the live engine to the per-tick hook.
            *engine_slot_ready.lock().unwrap() = Some(ctx.reactive_engine.clone());

            let driver: Arc<dyn UserDriver> = Arc::new(SimUserDriver {
                app_ptr: app_ptr,
                window,
                bounds: bounds.clone(),
                engine: ctx.reactive_engine.clone(),
                rt_handle: rt_handle.clone(),
                interaction_tx: interaction_tx.clone(),
            });
            Some(PbtReadyResult {
                driver: Some(driver),
                frontend_engine: Some(ctx.reactive_engine.clone()),
                frontend_geometry: Some(Box::new(bounds.clone())),
                frontend_visual_state: None,
            })
        };

        // E4 per-tick composed-catalog hook. OPT-IN via `HOLON_PBT_WINDOWED_CATALOG=1`
        // (the full catalog + a forced window settle every tick is a real cost — H-B4 —
        // so default-off keeps existing windowed runs fast; the `E2ESut` per-step
        // `check_invariants` still runs in parallel, so this is purely additive). When
        // on, each post-StartApp tick: settle the window, build the windowed `CapMap`
        // (`window_focus_wide` over this window's geometry + engine) and a ref `CapMap`
        // from the LIVE `ReferenceState`, then run the composed catalog over them and
        // panic on failures.
        let catalog_enabled = std::env::var("HOLON_PBT_WINDOWED_CATALOG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let bounds_tick = self.bounds.clone();
        let mut tick_idx: usize = 0;
        let per_tick = move |_: &mut holon_integration_tests::pbt::E2ESut,
                             ref_state: &ReferenceState| {
            if !catalog_enabled {
                return;
            }
            // B0 finding: `random_pbt_sim::run` keeps a multi-thread `window_rt.enter()`
            // guard active for the whole replay (`random_pbt_sim.rs:102`), so a tokio
            // runtime is ENTERED on this gpui thread at the hook point. That makes both
            // `Handle::block_on` and `block_in_place` panic (the thread is not a worker,
            // just a context-entered external thread). The escape hatch is
            // `futures::executor::block_on`: it is runtime-agnostic — it polls the future
            // on this thread to completion while any tokio primitive the invariants await
            // still resolves against the entered multi-thread `window_rt` (worker threads
            // make progress). The window is settled to a fixed point BEFORE this, so the
            // reads are over already-rendered state and blocking the gpui thread here
            // needs no further pumping.
            if tick_idx == 0 {
                eprintln!(
                    "[B0] windowed per-tick catalog ON; ambient_runtime_entered={} \
                     — driving via futures::executor::block_on",
                    tokio::runtime::Handle::try_current().is_ok()
                );
            }
            tick_idx += 1;

            let t_settle = std::time::Instant::now();
            settle_to_fixed_point(unsafe { &*app_ptr }, &bounds_tick, 500);
            let settle_ms = t_settle.elapsed().as_millis();

            let engine = engine_slot
                .lock()
                .unwrap()
                .clone()
                .expect("per-tick hook fired before StartApp populated the engine slot");
            let sut = window_focus_wide(Box::new(bounds_tick.clone()), engine);
            // Build the ref CapMap from the LIVE reference state (not a fresh/seeded one)
            // so the displayed-text/bounds checks compare against what the run actually
            // expects — load-bearing, not vacuous.
            let ref_caps =
                reference_state_ref_caps(Resolved::identity(ref_state.clone()).map(Arc::new));

            let t_check = std::time::Instant::now();
            let report = futures::executor::block_on(run_selected(
                &composed_invariant_catalog(),
                &sut,
                &ref_caps,
            ));
            let check_ms = t_check.elapsed().as_millis();
            if std::env::var("HOLON_PBT_WINDOWED_CATALOG_TIMING").is_ok() {
                eprintln!(
                    "[windowed-catalog] tick={tick_idx} settle_ms={settle_ms} \
                     check_ms={check_ms} ran={} failures={}",
                    report.ran_ids().len(),
                    report.failures().len()
                );
            }
            // Non-vacuity floor: the windowed geometry invariant MUST have been selected
            // and run over the live window — else a mis-built ref silently deselected
            // everything and `failures.is_empty()` is vacuously true. Mirrors the harness
            // `REQUIRED_INVARIANTS` discipline. (`window_focus_wide` + live ref ⇒ this
            // runs; the deterministic `gpui_window_slice` test proves it bites under a
            // planted fault — the per-tick loop reuses that exact `run_selected` path.)
            assert!(
                report.ran_ids().contains(&"inv-frontend-bounds-rendered"),
                "windowed per-tick non-vacuity: inv-frontend-bounds-rendered must run \
                 each tick (ran={:?}, deselected={:?})",
                report.ran_ids(),
                report.deselected.iter().map(|d| d.0).collect::<Vec<_>>()
            );
            assert!(
                report.failures().is_empty(),
                "windowed per-tick catalog diverged at tick {tick_idx}: {:?}",
                report.failures()
            );
        };

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replay_fixture_with_driver_sync_callback(
                wiring,
                steps,
                on_ready,
                per_tick,
                seen_counter,
            )
            .expect("sim replay setup failed");
        }))
    }
}
