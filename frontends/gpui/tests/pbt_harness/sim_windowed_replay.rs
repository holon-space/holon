//! `SimUserDriver` — the TestPlatform-backed `UserDriver` (direct gpui
//! dispatch, no real NSWindow). The composed windowed harness
//! (`windowed_wide.rs`, the 4b loop, `gpui_window_slice`,
//! `gpui_compose_sut_windowed`) constructs it over an already-launched, settled
//! window.
//!
//! Increment 4c: the phased `SimReplayer` replay host (and its
//! `HOLON_PBT_WINDOWED_CATALOG` opt-in per-tick check) was DELETED — the
//! composed path runs the full catalog every tick as the primary check inside
//! `ComposedSut::check_invariants`, so the opt-in twin became redundant.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::Keystroke;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_api::EntityUri;
use holon_api::KeyChord;
use holon_api::Region;
use holon_api::Value;
use holon_frontend::OperationIntent;
use holon_frontend::ReactiveViewModel;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;

/// One sim pump cycle: real tokio time for backend watchers, drain gpui
/// tasks (test builds draw dirty windows inside flush_effects), fire fake
/// timers, drain again, promote staged bounds.
fn pump_cycle(app: &HeadlessAppContext, bounds: &BoundsRegistry) {
    // Real wall-clock pause: the backend's multi-thread tokio runtime
    // advances on its own worker threads — no block_on needed (and driver
    // methods may already be inside a tokio context where block_on panics).
    std::thread::sleep(Duration::from_millis(10));
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(500));
    app.run_until_parked();
    bounds.flush();
}

/// How many [`pump_cycle`]s a bounds lookup waits for a frame that paints the
/// entity before failing loud.
///
/// Counted in CYCLES, not wall time, because `pump_cycle` advances the app's
/// FAKE CLOCK 500ms per iteration: a wall-clock deadline is a clock-injection
/// rate, so raising it changes what the tests under it simulate rather than
/// just how long they wait.
///
/// Eight is the settle chain, not a tuning knob. A re-created shell paints its
/// rows on the next signal tick — two frames — and a gesture that also mutates
/// has to clear CDC → Loro → projection first, which the harness elsewhere
/// budgets at a handful of cycles. Eight leaves ~4x headroom over the observed
/// need while keeping the miss cost bounded and deterministic.
const BOUNDS_WAIT_PUMP_CYCLES: usize = 8;

/// How many times [`SimUserDriver::send_key_chord`] re-clicks when the click
/// lands but focus does not move. Counted in ATTEMPTS because each attempt now
/// costs up to `BOUNDS_WAIT_PUMP_CYCLES` + the landing poll; the wall-clock
/// budget this replaced was written when the click point resolved instantly and
/// had silently narrowed to a single attempt.
const CHORD_FOCUS_CLICK_ATTEMPTS: usize = 3;

// ---------------------------------------------------------------------------
// SimUserDriver — UserDriver over direct TestPlatform dispatch
// ---------------------------------------------------------------------------

/// Direct-dispatch UserDriver for TestPlatform. Holds a raw pointer to a
/// harness-owned `HeadlessAppContext`. SAFETY: the owner outlives all
/// SimUserDriver instances; all access is from one thread.
pub(crate) struct SimUserDriver {
    app_ptr: *const HeadlessAppContext,
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
    /// caller must keep the `HeadlessAppContext` `app_ptr` points at alive and
    /// untouched for the driver's lifetime, and use the driver only from the
    /// gpui thread (the same contract `with_windowed_wide_sut` upholds). Used
    /// by the windowed composition slice (`gpui_window_slice`) and the
    /// composed windowed harness.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app_ptr: *const HeadlessAppContext,
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
        let app = unsafe { &mut *(self.app_ptr as *mut HeadlessAppContext) };
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

    /// Mirror `GpuiUserDriver::text_center`: the center of the entity's TEXT
    /// element, where a caret-seating click has to land.
    ///
    /// `bounds_center_f32`'s first key, `render-entity-{id}`, records no
    /// bounds, so resolution falls through to `selectable-{id}` — a 16x24
    /// bullet drag handle whose click never seats a caret. The text element is
    /// what a user aims at, exists focused (`editable_text`) and unfocused
    /// (`rendered_text`), and sits inside the focus wrapper, so the click
    /// reaches both handlers.
    fn text_center(&self, entity_id: &EntityUri) -> Option<(f32, f32)> {
        self.pump();
        let eid = entity_id.as_str();
        let elements = self.bounds.all_elements();
        ["editable_text", "rendered_text"]
            .into_iter()
            .find_map(|want| {
                elements
                    .iter()
                    .find(|(_, i)| {
                        i.entity_id.as_deref() == Some(eid)
                            && i.widget_type.as_ref() == want
                            && i.has_visible_area()
                    })
                    .map(|(_, i)| i.center())
            })
    }

    /// What the window painted, from the entity's point of view — the census a
    /// "not in bounds" failure needs to be actionable. Distinguishes the three
    /// ways the lookup can come up empty: the entity is painted but clipped to
    /// zero visible area, it is painted with no text element (so a
    /// caret-seating click has nothing to aim at), or the projection never
    /// mounted it at all.
    fn miss_census(&self, entity_id: &EntityUri) -> String {
        let eid = entity_id.as_str();
        let elements = self.bounds.all_elements();
        let mine: Vec<String> = elements
            .iter()
            .filter(|(_, i)| i.entity_id.as_deref() == Some(eid))
            .map(|(id, i)| {
                format!(
                    "{id}[{}] {}x{} @{},{}{}",
                    i.widget_type,
                    i.width,
                    i.height,
                    i.x,
                    i.y,
                    if i.has_visible_area() { "" } else { " CLIPPED" }
                )
            })
            .collect();
        let mut painted_entities: Vec<&str> = elements
            .iter()
            .filter_map(|(_, i)| i.entity_id.as_deref())
            .collect();
        painted_entities.sort_unstable();
        painted_entities.dedup();
        let mut full: Vec<String> = elements
            .iter()
            .map(|(id, i)| {
                format!(
                    "{id}|{}|ent={:?}|parent={:?}|txt={:?}|{}x{}@{},{}",
                    i.widget_type,
                    i.entity_id.as_deref(),
                    i.parent_id.as_deref(),
                    i.displayed_text.as_deref(),
                    i.width,
                    i.height,
                    i.x,
                    i.y
                )
            })
            .collect();
        full.sort();
        format!(
            "elements={}, engine_focus={:?}, entities painted={:?}, elements bound to \
             {eid}={:?}\nFULL:\n{}",
            elements.len(),
            self.engine.focused_block(),
            painted_entities,
            mine,
            full.join("\n")
        )
    }

    /// The click point for `entity_id`, WAITING for a frame that actually
    /// paints it. Fails loud at [`BOUNDS_WAIT_PUMP_CYCLES`] with
    /// [`Self::miss_census`].
    ///
    /// The main panel's virtualized `ReactiveShell` is re-created with an EMPTY
    /// item vec on every projection rebuild and fills only on its next signal
    /// tick, so a one-shot read lands on a blank panel whenever it falls in
    /// that window and the gesture fails on a row that is on screen a frame
    /// later. Every bounds-taking verb on this driver routes through here.
    fn click_point_when_painted(
        &self,
        entity_id: &EntityUri,
        prefer_text: bool,
    ) -> Result<(f32, f32), anyhow::Error> {
        let app = unsafe { &*self.app_ptr };
        for _ in 0..BOUNDS_WAIT_PUMP_CYCLES {
            let center = if prefer_text {
                self.text_center(entity_id)
            } else {
                None
            }
            .or_else(|| self.bounds_center_f32(entity_id));
            if let Some(center) = center {
                return Ok(center);
            }
            pump_cycle(app, &self.bounds);
        }
        anyhow::bail!(
            "entity {entity_id} not in bounds after {BOUNDS_WAIT_PUMP_CYCLES} pump cycles ({})",
            self.miss_census(entity_id)
        )
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

    /// Click at a window point that belongs to no tracked entity — the gesture
    /// a user makes when they tap the dimmed area beside an open overlay
    /// drawer.
    pub fn click_point(&self, x: f32, y: f32) {
        self.raw_click(Point {
            x: Pixels::from(x),
            y: Pixels::from(y),
        });
    }

    /// Move the pointer over a window point and settle. This is the whole of a
    /// hover gesture: gpui resolves hover from the window's tracked mouse
    /// position, which only a `MouseMove` updates.
    pub fn hover_point(&self, x: f32, y: f32) {
        let pos = Point {
            x: Pixels::from(x),
            y: Pixels::from(y),
        };
        self.update_and_settle(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::MouseMoveEvent {
                            position: pos,
                            pressed_button: None,
                            modifiers: Default::default(),
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
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
        use holon_gpui::user_driver::is_modifier;
        use holon_gpui::user_driver::keystroke_name;
        use holon_gpui::user_driver::modifier_name;
        let id = entity_id.as_str();

        // Click-to-focus with re-click until focused_block lands on the
        // target — a single click can land on a neighbor row when an async
        // commit shifts bounds mid-click.
        let mut attempts_left = CHORD_FOCUS_CLICK_ATTEMPTS;
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
            let (cx, cy) = self
                .click_point_when_painted(entity_id, false)
                .map_err(|e| anyhow::anyhow!("send_key_chord click-to-focus: {e:#}"))?;
            self.raw_click(Point {
                x: Pixels::from(cx),
                y: Pixels::from(cy),
            });
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
            attempts_left -= 1;
            if attempts_left == 0 {
                anyhow::bail!(
                    "send_key_chord: click on {entity_id} never moved focused_block to it in \
                     {CHORD_FOCUS_CLICK_ATTEMPTS} attempts — refusing to press {chord:?} into the \
                     wrong editor"
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
                         within 2s of click-to-focus — refusing to press {chord:?} into whatever \
                         editor still holds it"
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

    /// A main-panel click is a caret-seating gesture, so it aims at the row's
    /// text (`text_center`); sidebar rows navigate instead and keep the alias
    /// chain. Falls back to the alias chain when the entity renders no text
    /// element (page shells, non-block handles).
    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<(), anyhow::Error> {
        let is_main = region
            .parse::<Region>()
            // ALLOW(unwrap_or): an unparseable region is the same "no sidebar
            // scoping" case `GpuiUserDriver::require_click_center` treats as main.
            .map(|r| r == Region::Main)
            .unwrap_or(true);
        let (cx, cy) = self.click_point_when_painted(entity_id, is_main)?;
        self.raw_click(Point {
            x: Pixels::from(cx),
            y: Pixels::from(cy),
        });
        Ok(())
    }

    /// Mirror `GpuiUserDriver::set_block_expanded`: click the chevron
    /// registered under `expand_toggle_id_for(target)` so the production
    /// handler flips the row's view-local `expanded` `Mutable<bool>`. Direction
    /// is ref-owned (see the trait doc), so the desired-state arg is unused —
    /// the chevron is a toggle and the ref only generates the state-changing
    /// direction.
    async fn set_block_expanded(&self, target: &EntityUri, _: bool) -> Result<(), anyhow::Error> {
        let target_str = target.as_str();
        let bare = target_str.strip_prefix("block:").unwrap_or(target_str);
        let element_id = holon_frontend::expand_toggle_id_for(bare);
        // Same frame-wait contract as `click_point_when_painted`: the chevron
        // lives on a main-panel row, so it is absent from any frame the
        // virtualized shell rebuilt empty.
        let app = unsafe { &*self.app_ptr };
        let mut found = None;
        for _ in 0..BOUNDS_WAIT_PUMP_CYCLES {
            self.pump();
            if let Some(info) = self.bounds.element_info(&element_id) {
                found = Some(info);
                break;
            }
            pump_cycle(app, &self.bounds);
        }
        let Some(info) = found else {
            anyhow::bail!(
                "set_block_expanded: chevron {element_id} not in bounds after \
                 {BOUNDS_WAIT_PUMP_CYCLES} pump cycles ({})",
                self.miss_census(target)
            );
        };
        let (cx, cy) = info.center();
        self.raw_click(Point {
            x: Pixels::from(cx),
            y: Pixels::from(cy),
        });
        Ok(())
    }

    /// Caret CLICK: identical gesture — the chevron is a toggle, so
    /// `set_block_expanded`'s direction arg is unused and its body IS the
    /// click (mirrors `GpuiUserDriver`).
    async fn click_expand_toggle(&self, target: &EntityUri) -> Result<(), anyhow::Error> {
        self.set_block_expanded(target, false).await
    }

    /// The trait default clicks the block ROW (which only focuses), never
    /// reaching the `state_toggle` glyph's `on_mouse_down`, so `task_state`
    /// never cycles — the windowed `toggle_state never landed` defect. The
    /// window's `state_toggle` elements register with NO `entity_id` and 0x0
    /// bounds, so they cannot be geometry-hit-tested by block. Resolve +
    /// dispatch the SAME `set_field` cycle intent the widget's `on_mouse_down`
    /// builds — off the resolved view tree, exactly as headless
    /// `ReactiveEngineDriver::cycle_state_toggle` does (the `StateToggle` NODE
    /// carries the block `entity_id`). Then pump so the projection the caller's
    /// landing-poll reads reflects the write.
    async fn cycle_state_toggle(
        &self,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<(), anyhow::Error> {
        let root = holon_api::root_layout_block_uri();
        let resolved = self.engine.snapshot_resolved(&root);
        let intent =
            holon_frontend::focus_path::state_toggle_cycle_intent(&resolved, entity_id, region)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cycle_state_toggle: no state_toggle node for {entity_id} in region \
                         {region} (target rendered no task toggle — not a visible task row?)"
                    )
                })?;
        self.engine.dispatch_intent_sync(intent).await?;
        self.pump();
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
                    // Move the pointer to the target first, mirroring
                    // `GpuiUserDriver::scroll_at`: gpui recomputes the scroll
                    // hit-test from the window's tracked mouse position, so the
                    // move keeps the wheel landing on the intended viewport.
                    window.dispatch_event(
                        gpui::MouseMoveEvent {
                            position: point,
                            pressed_button: None,
                            modifiers: Default::default(),
                        }
                        .to_platform_input(),
                        cx,
                    );
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
        // Same frame-wait as every other bounds-taking verb, and a MISS IS AN
        // ERROR: this used to return `Ok(())`, so a scroll aimed at a frame the
        // shell had rebuilt empty became a silent no-op and the caller went on
        // believing it had scrolled.
        let (cx, cy) = self
            .click_point_when_painted(entity_id, false)
            .map_err(|e| anyhow::anyhow!("scroll_entity: {e:#}"))?;
        self.scroll_at(cx, cy, dx, dy).await
    }

    async fn drop_entity(
        &self,
        _: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
    ) -> Result<bool, anyhow::Error> {
        let src = self
            .click_point_when_painted(source_id, false)
            .map(|(x, y)| Point {
                x: Pixels::from(x),
                y: Pixels::from(y),
            })
            .map_err(|e| anyhow::anyhow!("drop_entity source: {e:#}"))?;
        let dst = self
            .click_point_when_painted(target_id, false)
            .map(|(x, y)| Point {
                x: Pixels::from(x),
                y: Pixels::from(y),
            })
            .map_err(|e| anyhow::anyhow!("drop_entity target: {e:#}"))?;
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
