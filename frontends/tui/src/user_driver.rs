//! TUI [`UserDriver`] that drives the real `app_handle_input_event`
//! pipeline.
//!
//! Translates each [`UserDriver`] verb into a sequence of synthetic
//! [`r3bl_tui::InputEvent`]s pushed through `input_tx` to the renderer
//! task in `tui_ui_pbt.rs`. The renderer's third `tokio::select!` arm
//! drains the channel and calls
//! [`crate::app_main::AppMain::app_handle_input_event`] exactly as r3bl would
//! in production. After every event the renderer re-renders and bumps
//! `render_seq` + notifies waiters; verbs await that barrier (and an
//! engine-quiescence barrier for chord-resolved ops) before returning.
//!
//! This is intentionally NOT field-for-field aligned with
//! `holon_gpui::user_driver::GpuiUserDriver` — GPUI's input pipeline is
//! mouse-and-keystroke; the TUI's is keyboard-only (no mouse arm in
//! `app_handle_input_event`). Verbs that have no keyboard equivalent
//! (`drop_entity`) document a `LIMITATION:` and stay on the engine
//! fast-path; verbs that ARE keyboard equivalents (`click_entity` →
//! navigate-and-Enter) override the trait default.
//!
//! Two engine fast-path verbs remain — `synthetic_dispatch` (test-setup
//! primitive: not a user action) and `drop_entity` (no keyboard DnD).
//! Both delegate to the inner [`ReactiveEngineDriver`], which also
//! provides the engine-quiescence barrier (`warm_for_block` /
//! `snapshot_emission_tick` / `wait_emissions_quiesced`) the TUI driver
//! needs after dispatching a chord through the input pipeline. The
//! TUI helpers `tokio::spawn` engine dispatches; a render barrier alone
//! is NOT proof the engine has settled.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use async_trait::async_trait;
use futures::channel::mpsc::Sender;
use holon_api::KeyChord;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::ReactiveEngineDriver;
use holon_frontend::user_driver::UserDriver;
use holon_mcp::server::InteractionCommand;
use r3bl_tui::InputEvent;
use r3bl_tui::Key as RKey;
use r3bl_tui::KeyPress;
use r3bl_tui::ModifierKeysMask;
use r3bl_tui::SpecialKey;

use crate::app_main::EditState;
use crate::app_main::NO_FOCUS;
use crate::render::RenderRegistry;

/// Channel-driven [`UserDriver`] for the TUI. See module docs.
pub struct TuiUserDriver {
    pub geometry: Arc<dyn GeometryProvider>,
    pub engine: Arc<ReactiveEngine>,

    /// Channel into the renderer task's `tokio::select!` loop. Each
    /// `InputEvent` triggers exactly one `app_handle_input_event` +
    /// `app_render` cycle on the renderer side.
    pub input_tx: tokio::sync::mpsc::Sender<InputEvent>,

    /// Shared with `TuiState` — written on every `app_render`. The driver
    /// reads it during `nav_to` to find the (region, index) of a target
    /// entity and to track focus convergence.
    pub last_registry: Arc<Mutex<RenderRegistry>>,
    /// Shared with `TuiState`. `usize::MAX` (== `NO_FOCUS`) when no
    /// selectable is focused yet; otherwise an index into
    /// `last_registry.selectables`.
    pub focus_index: Arc<AtomicUsize>,
    /// Shared with `TuiState`. The driver inspects this in `type_text`
    /// (post-Enter, must be `Some` else we bail loud — INV-LOUD-FAILURE)
    /// to detect "Enter didn't actually open edit mode" before typing
    /// chars that would otherwise route into the navigation handler.
    pub edit_state: Arc<Mutex<Option<EditState>>>,

    /// Bumped by the renderer after every `app_render`. The driver awaits
    /// `render_seq > pre` to barrier on render completion.
    pub render_seq: Arc<AtomicU64>,
    /// Notified by the renderer after every `app_render`. Paired with
    /// `render_seq` for the lost-wakeup-safe `await_render` (see method
    /// docs).
    pub render_notify: Arc<tokio::sync::Notify>,

    /// MCP-only channel. Populated by `setup_interaction_pump` for parity
    /// with the GPUI driver shape; the PBT path does NOT send through this
    /// channel (PBT input goes via `input_tx`). MCP clients still need
    /// this channel to deliver `InteractionEvent`s with response
    /// oneshots.
    pub interaction_tx: Sender<InteractionCommand>,

    /// Engine fast-path delegate. Carries:
    /// - `synthetic_dispatch` (test-setup) and `drop_entity` (no keyboard DnD)
    ///   targets.
    /// - The engine-quiescence barrier (`warm_for_block`,
    ///   `snapshot_emission_tick`, `wait_emissions_quiesced`) — the TUI helpers
    ///   `tokio::spawn` engine dispatches, so a render barrier alone is
    ///   insufficient. The driver routes chord-resolved ops through the real
    ///   input pipeline (NOT through `inner`'s `send_key_chord`), but then
    ///   waits on `inner`'s router to confirm the spawned dispatches settled.
    /// - `resolve_key_chord` (via the `UserDriver` trait), used to translate a
    ///   fixture chord into the operation name the TUI then binds to its own
    ///   keystroke set.
    inner: ReactiveEngineDriver,
}

impl TuiUserDriver {
    pub fn new(
        engine: Arc<ReactiveEngine>,
        geometry: Arc<dyn GeometryProvider>,
        input_tx: tokio::sync::mpsc::Sender<InputEvent>,
        last_registry: Arc<Mutex<RenderRegistry>>,
        focus_index: Arc<AtomicUsize>,
        edit_state: Arc<Mutex<Option<EditState>>>,
        render_seq: Arc<AtomicU64>,
        render_notify: Arc<tokio::sync::Notify>,
        interaction_tx: Sender<InteractionCommand>,
    ) -> Self {
        let inner = ReactiveEngineDriver::new(engine.clone());
        Self {
            geometry,
            engine,
            input_tx,
            last_registry,
            focus_index,
            edit_state,
            render_seq,
            render_notify,
            interaction_tx,
            inner,
        }
    }

    /// Push a single `InputEvent` to the renderer. The send fails (loud)
    /// if the renderer task has dropped its receiver — that means the
    /// PBT thread is unwinding and we should propagate.
    async fn send_input(&self, ev: InputEvent) -> Result<()> {
        self.input_tx
            .send(ev)
            .await
            .map_err(|_| anyhow!("renderer input channel closed; renderer task likely panicked"))
    }

    /// Wait until `render_seq` advances past the value at entry, with
    /// the lost-wakeup fix: register interest on `render_notify` BEFORE
    /// the predicate check via `Notified::enable()`. Without this, a
    /// notify between `notified()` creation and the eventual poll inside
    /// `tokio::time::timeout` is silently lost (documented tokio gotcha).
    /// The seq counter handles spurious wakeups.
    async fn await_render(&self, deadline: Instant) -> Result<()> {
        let pre = self.render_seq.load(Ordering::Acquire);
        loop {
            let notified = self.render_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.render_seq.load(Ordering::Acquire) > pre {
                return Ok(());
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                bail!("await_render timeout (pre={pre})");
            }
            tokio::time::timeout(timeout, notified)
                .await
                .context("render_notify timed out")?;
        }
    }

    /// Wait until the next render has captured the post-input state AND
    /// the engine has settled any spawned dispatches the input may have
    /// triggered (INV-DISPATCH-COMPLETED).
    ///
    /// `app_main`'s helpers (`dispatch_block_op_on_focused`,
    /// `enter_pressed`'s `state.engine.dispatch_intent`,
    /// `handle_edit_input`'s `split_block`/`join_block`) all
    /// `state.rt_handle.spawn(...)` the actual engine dispatch; control
    /// returns from `app_handle_input_event` immediately, before the
    /// engine has done anything. A single render-completion barrier
    /// would acknowledge "input handled" but read pre-mutation state.
    async fn await_chord_settled(
        &self,
        root_block_id: &holon_api::EntityUri,
        deadline: Instant,
    ) -> Result<()> {
        // 1. Make sure the inner router is warmed for this root so the quiescence wait
        //    can detect emissions. Idempotent.
        self.inner
            .warm_for_block(root_block_id)
            .await
            .context("await_chord_settled: failed to warm inner router")?;

        let pre_tick = self.inner.snapshot_emission_tick();

        // 2. Render barrier — the input has now been processed by
        //    `app_handle_input_event` and any synchronous mutations are visible. Async
        //    tokio::spawn'd engine dispatches may still be in flight.
        self.await_render(deadline).await?;

        // 3. Engine-quiescence barrier — wait for the emission tick to advance past
        //    `pre_tick` and then stay stable for the configured window. Catches the
        //    spawn-vs-render race.
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            bail!("await_chord_settled: no time left for quiescence wait");
        }
        match tokio::time::timeout(timeout, self.inner.wait_emissions_quiesced(pre_tick)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e).context("await_chord_settled: engine did not quiesce"),
            Err(_) => bail!("await_chord_settled: engine quiesce wait timed out"),
        }

        // 4. One more render so we observe the post-quiesce state.
        self.await_render(deadline).await?;
        Ok(())
    }

    /// Region index of whatever selectable is currently focused, or
    /// `None` if `focus_index == NO_FOCUS` or out-of-bounds (post-CDC
    /// stale index).
    fn current_region(&self) -> Option<usize> {
        let idx = self.focus_index.load(Ordering::Acquire);
        if idx == NO_FOCUS {
            return None;
        }
        let reg = self.last_registry.lock().unwrap();
        reg.selectables.get(idx).map(|s| s.region)
    }

    /// Look up the region the target entity lives in. Returns `None`
    /// if the entity isn't currently in the registry.
    fn lookup_region(&self, entity_id: &holon_api::EntityUri) -> Option<usize> {
        let reg = self.last_registry.lock().unwrap();
        reg.selectables
            .iter()
            .find(|s| s.entity_id == entity_id.as_str())
            .map(|s| s.region)
    }

    /// Diagnostic — list of (region, entity_id) currently in the
    /// registry. Used by `nav_to`'s error message when the target is
    /// missing.
    fn registry_snapshot_summary(&self) -> String {
        let reg = self.last_registry.lock().unwrap();
        let entries: Vec<String> = reg
            .selectables
            .iter()
            .map(|s| format!("({}:{})", s.region, s.entity_id))
            .collect();
        entries.join(", ")
    }

    /// Navigate keyboard focus to `target_entity_id`. Emits Tab to hop
    /// regions, then Down to walk within the region. Re-snapshots the
    /// registry inside each loop iteration so registry mutations during
    /// CDC don't desync the walk; bails loud if the target ever
    /// disappears or the walk exceeds twice the region size.
    async fn nav_to(&self, target_entity_id: &holon_api::EntityUri) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);

        let target_region = self.lookup_region(target_entity_id).with_context(|| {
            format!(
                "nav_to: entity {target_entity_id} not in registry; available: [{}]",
                self.registry_snapshot_summary()
            )
        })?;

        // 1. Hop regions until cur_region == target_region.
        let mut tab_steps = 0;
        while self.current_region() != Some(target_region) {
            if Instant::now() >= deadline {
                bail!(
                    "nav_to: region hop timeout (target_region={target_region}, \
                     target={target_entity_id})"
                );
            }
            if tab_steps > 16 {
                bail!(
                    "nav_to: too many region hops without convergence \
                     (target_region={target_region})"
                );
            }
            self.send_input(InputEvent::Keyboard(KeyPress::Plain {
                key: RKey::SpecialKey(SpecialKey::Tab),
            }))
            .await?;
            self.await_render(deadline).await?;
            tab_steps += 1;
        }

        // 2. Walk Down within the target region until focus matches. advance_focus
        //    cycles modularly within the region; bound by 2 * max-observed-region-size
        //    + 1 to allow for one full wraparound + slack for legitimate registry
        //    deltas.
        let mut max_seen = 0usize;
        let mut steps = 0usize;
        loop {
            if Instant::now() >= deadline {
                bail!("nav_to: walk timeout (target={target_entity_id})");
            }
            let (focused_eid, region_size, target_still_present) = {
                let reg = self.last_registry.lock().unwrap();
                let idx = self.focus_index.load(Ordering::Acquire);
                let focused = reg.selectables.get(idx).map(|s| s.entity_id.clone());
                let region_size = reg
                    .selectables
                    .iter()
                    .filter(|s| s.region == target_region)
                    .count();
                let target_present = reg
                    .selectables
                    .iter()
                    .any(|s| s.entity_id == target_entity_id.as_str());
                (focused, region_size, target_present)
            };
            if !target_still_present {
                bail!(
                    "nav_to: target {target_entity_id} disappeared from registry mid-walk after \
                     {steps} steps"
                );
            }
            if focused_eid.as_deref() == Some(target_entity_id.as_str()) {
                return Ok(());
            }
            max_seen = max_seen.max(region_size);
            if steps > 2 * max_seen + 1 {
                bail!(
                    "nav_to: walked past target {target_entity_id} ({steps} steps, max region \
                     size {max_seen})"
                );
            }
            self.send_input(InputEvent::Keyboard(KeyPress::Plain {
                key: RKey::SpecialKey(SpecialKey::Down),
            }))
            .await?;
            self.await_render(deadline).await?;
            steps += 1;
        }
    }
}

/// Translate a chord's resolved operation name into the TUI keystroke
/// sequence that triggers that op in `app_handle_input_event`. This is
/// the SINGLE per-frontend translation — chord→op resolution happens
/// upstream via the engine binding registry (`bubble_input_oneshot`).
///
/// Returns `Err` (per CLAUDE.md fail-loud policy) when the op has no
/// TUI binding — a fixture introducing a new op without adding a TUI
/// binding will fail loud at runtime, not silently no-op.
fn tui_keystrokes_for_op(
    op_name: &str,
    extra_params: &HashMap<String, Value>,
) -> Result<Vec<InputEvent>> {
    use SpecialKey::*;
    let plain = |k: RKey| InputEvent::Keyboard(KeyPress::Plain { key: k });
    let with_mods = |k: RKey, mask: ModifierKeysMask| {
        InputEvent::Keyboard(KeyPress::WithModifiers { key: k, mask })
    };

    Ok(match op_name {
        // app_main.rs:207-221 — Ctrl+T cycles task state.
        "cycle_task_state" => vec![with_mods(
            RKey::Character('t'),
            ModifierKeysMask::default().with_ctrl(),
        )],
        // app_main.rs:229-243 — Alt+i indents (TUI's Tab is region-switch).
        "indent" => vec![with_mods(
            RKey::Character('i'),
            ModifierKeysMask::default().with_alt(),
        )],
        // app_main.rs:244-258 — Alt+o outdents.
        "outdent" => vec![with_mods(
            RKey::Character('o'),
            ModifierKeysMask::default().with_alt(),
        )],
        // app_main.rs:275-284 — leader Space, then Up.
        "move_up" => vec![plain(RKey::Character(' ')), plain(RKey::SpecialKey(Up))],
        // app_main.rs:285-294 — leader Space, then Down.
        "move_down" => vec![plain(RKey::Character(' ')), plain(RKey::SpecialKey(Down))],
        // app_main.rs:393-400 — Enter on a Selectable activates the
        // bound click intent; Enter on a Block opens edit mode.
        "editor_focus" => vec![plain(RKey::SpecialKey(Enter))],
        // app_main.rs:986-1010 — Ctrl+x in edit mode splits at the
        // current cursor. Sequence: Enter (open edit), Home (cursor=0),
        // Right × position (advance cursor codepoint by codepoint),
        // Ctrl+x (dispatch split). The handler reads `edit_state.cursor`
        // at dispatch time so the position must be set via keystrokes,
        // not via params.
        "split_block" => {
            let position = extra_params
                .get("position")
                .and_then(|v| match v {
                    Value::Integer(i) => Some(*i),
                    _ => None,
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "tui_keystrokes_for_op: split_block requires 'position' (Integer) in \
                         extra_params"
                    )
                })?;
            if position < 0 {
                bail!("tui_keystrokes_for_op: split_block position must be >= 0, got {position}");
            }
            let mut events = Vec::with_capacity(3 + position as usize);
            events.push(plain(RKey::SpecialKey(Enter))); // open edit mode
            events.push(plain(RKey::SpecialKey(Home))); // cursor → 0
            for _ in 0..position {
                events.push(plain(RKey::SpecialKey(Right))); // advance one
                // codepoint
            }
            events.push(with_mods(
                RKey::Character('x'),
                ModifierKeysMask::default().with_ctrl(),
            ));
            events
        }
        // app_main.rs:1016-1037 — Backspace in edit mode at cursor=0
        // joins with the previous sibling. Sequence: Enter (open edit),
        // Home (cursor=0), Backspace (dispatch join). The cursor=0
        // precondition is enforced by the production handler — we just
        // make sure Home ran first.
        "join_block" => vec![
            plain(RKey::SpecialKey(Enter)), // open edit mode
            plain(RKey::SpecialKey(Home)),  // cursor → 0 (precondition for join)
            plain(RKey::SpecialKey(Backspace)),
        ],
        other => bail!(
            "tui_keystrokes_for_op: no TUI keystroke binding for op '{other}'. Either add a \
             binding in app_main.rs and update this table, or dispatch via synthetic_dispatch."
        ),
    })
}

/// Maps a navigation [`holon_api::Region`] to the top-level `columns`
/// slot index the renderer tags rows with (see `render_columns` in
/// `crate::render`). The default layout is `[left-sidebar, main-panel,
/// right-sidebar]`, so the slot order matches the panel order. Keep this
/// in sync with the layout's column ordering.
fn region_columns_slot(region: holon_api::Region) -> usize {
    match region {
        holon_api::Region::LeftSidebar => 0,
        holon_api::Region::Main => 1,
        holon_api::Region::RightSidebar => 2,
    }
}

/// Entity ids painted under columns `slot` with non-zero extent, parsed to
/// URIs. Backs `entities_in_region`; split out so it's testable against a
/// hand-built `RenderRegistry` without standing up a full driver.
fn registry_entities_in_slot(registry: &RenderRegistry, slot: usize) -> Vec<holon_api::EntityUri> {
    registry
        .selectables
        .iter()
        .filter(|r| r.region == slot && r.rows > 0 && r.cols > 0)
        // ALLOW(filter_map_ok): the registry holds whatever id the builder
        // tagged (raw row ids without a scheme); the trait contract is
        // `Vec<EntityUri>` and non-URI ids aren't addressable anyway.
        .filter_map(|r| holon_api::EntityUri::parse(&r.entity_id).ok()) // ALLOW(ok): same
        .collect()
}

/// The click intent baked for `entity_id` at shadow-build time, if a
/// selectable in the registry targets it. Backs `click_intent_of`.
fn registry_click_intent(registry: &RenderRegistry, entity_id: &str) -> Option<OperationIntent> {
    registry
        .selectables
        .iter()
        .find(|r| r.entity_id == entity_id)
        .and_then(|r| r.intent.clone())
}

/// Translate a `send_raw_keystroke` keystroke string + modifier names
/// into a single `InputEvent`. Used for the atomic-editor PBT path
/// (TypeChars, MoveCursor, DeleteBackward, PressKey, Blur) which calls
/// `send_raw_keystroke(keystroke, &modifiers)` instead of going through
/// chord resolution.
///
/// `keystroke` is one of: `home`, `end`, `up`, `down`, `left`, `right`,
/// `pageup`, `pagedown`, `tab`, `enter`, `backspace`, `escape`, OR a
/// single-char string. `modifiers` is a slice of `ctrl`/`alt`/`shift`/
/// `cmd`. `cmd` is **not** a TUI modifier — bail loud if we see it
/// (per CLAUDE.md fail-loud policy; the PBT shouldn't be sending
/// raw cmd-keystrokes anyway, that's send_key_chord territory).
fn raw_keystroke_to_input_event(keystroke: &str, modifiers: &[&str]) -> Result<InputEvent> {
    use SpecialKey::*;

    let mut mask = ModifierKeysMask::default();
    let mut have_mods = false;
    for m in modifiers {
        match *m {
            "ctrl" => {
                mask = mask.with_ctrl();
                have_mods = true;
            }
            "alt" => {
                mask = mask.with_alt();
                have_mods = true;
            }
            "shift" => {
                mask = mask.with_shift();
                have_mods = true;
            }
            "cmd" => bail!(
                "raw_keystroke_to_input_event: 'cmd' modifier has no TUI equivalent. Use \
                 send_key_chord for semantic ops (cycle_task_state, etc.)."
            ),
            other => bail!("raw_keystroke_to_input_event: unknown modifier '{other}'"),
        }
    }

    let key = match keystroke {
        "home" => RKey::SpecialKey(Home),
        "end" => RKey::SpecialKey(End),
        "up" => RKey::SpecialKey(Up),
        "down" => RKey::SpecialKey(Down),
        "left" => RKey::SpecialKey(Left),
        "right" => RKey::SpecialKey(Right),
        "pageup" => RKey::SpecialKey(PageUp),
        "pagedown" => RKey::SpecialKey(PageDown),
        "tab" => RKey::SpecialKey(Tab),
        "enter" => RKey::SpecialKey(Enter),
        "backspace" => RKey::SpecialKey(Backspace),
        "escape" => RKey::SpecialKey(Esc),
        s if s.chars().count() == 1 => RKey::Character(s.chars().next().unwrap()),
        other => bail!(
            "raw_keystroke_to_input_event: unrecognised keystroke '{other}' (expected named \
             special key or single character)"
        ),
    };

    Ok(if have_mods {
        InputEvent::Keyboard(KeyPress::WithModifiers { key, mask })
    } else {
        InputEvent::Keyboard(KeyPress::Plain { key })
    })
}

#[async_trait]
impl UserDriver for TuiUserDriver {
    /// Test-setup primitive — NOT a user action. Used for initial seeding
    /// (creating fixture documents, setting peer config) before the user
    /// "starts interacting". Bypasses the input pipeline by design.
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    /// Main verb. Resolves the chord against the engine binding registry
    /// (via the `UserDriver::resolve_key_chord` default →
    /// `bubble_input_oneshot`) to get the operation name, then emits the
    /// TUI keystroke sequence bound to that op. After the chord, awaits
    /// both render and engine quiescence (INV-DISPATCH-COMPLETED) so the
    /// PBT step reads post-mutation state, not pre-mutation.
    async fn send_key_chord(
        &self,
        root_block_id: &holon_api::EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &holon_api::EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        // `extra_params` is now threaded into `tui_keystrokes_for_op`
        // for ops like split_block (`position`); the per-op handler
        // either consumes it or rejects entries it doesn't understand.
        // Earlier this function bailed unconditionally on non-empty
        // extras — see `frontends/tui/TODO.md` D1/D2.

        // Make sure the inner router is warmed for chord resolution.
        // resolve_key_chord falls back to `bubble_input_oneshot` against
        // the snapshot tree if the router isn't ready, but warming first
        // is more reliable.
        self.inner
            .warm_for_block(root_block_id)
            .await
            .context("send_key_chord: failed to warm router for chord resolution")?;

        let op_name = match self
            .inner
            .resolve_key_chord(root_block_id, root_tree, entity_id, chord)
        {
            Some(name) => name,
            // Chord not bound on this entity — same contract as the
            // ReactiveEngineDriver returns Ok(false): "I tried, no
            // match, caller should fall back".
            None => return Ok(false),
        };

        // Find the entity in the registry and walk to it via keyboard nav.
        self.nav_to(entity_id).await?;

        let events = tui_keystrokes_for_op(&op_name, &extra_params).with_context(|| {
            format!(
                "send_key_chord: op '{op_name}' (resolved from chord {chord:?}) has no TUI \
                 keystroke binding"
            )
        })?;

        let deadline = Instant::now() + Duration::from_secs(5);
        for ev in events {
            self.send_input(ev).await?;
            self.await_render(deadline).await?;
        }
        self.await_chord_settled(root_block_id, deadline).await?;
        Ok(true)
    }

    /// Override: TUI's keyboard-equivalent of "click this thing" is
    /// "navigate to it and press Enter". On a Selectable region, Enter
    /// dispatches the bound click intent (typically `navigation_focus`
    /// for sidebar entries); on a Block region, Enter opens edit mode.
    /// Either way the mirror of the GPUI default `click_entity`
    /// (which dispatches `navigation::editor_focus`).
    async fn click_entity(&self, entity_id: &holon_api::EntityUri, _: &str) -> Result<()> {
        // The bare `_` region argument is GPUI-relevant (which screen
        // region the click happened in); the TUI's keyboard nav
        // resolves the region from the registry directly. Bare `_`
        // (not `_region`) per the `archlint no-underscore-params` rule.
        self.nav_to(entity_id).await?;
        self.send_input(InputEvent::Keyboard(KeyPress::Plain {
            key: RKey::SpecialKey(SpecialKey::Enter),
        }))
        .await?;

        // We don't know what root the click was on, but for the
        // quiescence wait we need a root block. Use the focused entity
        // itself — the router warms on any block, the tick advances on
        // any emission, so this is safe even if `entity_id` isn't the
        // logical root.
        let deadline = Instant::now() + Duration::from_secs(5);
        self.await_chord_settled(entity_id, deadline).await
    }

    /// LIMITATION: TUI has no DnD path through `app_handle_input_event`
    /// (no mouse arm, no keyboard equivalent). Stays on the engine
    /// fast-path so the resulting operation is still exercised, just
    /// not via the input pipeline. Documented per project's fail-loud
    /// policy: this path is explicit, not silent.
    async fn drop_entity(
        &self,
        root_block_id: &holon_api::EntityUri,
        source_id: &holon_api::EntityUri,
        target_id: &holon_api::EntityUri,
    ) -> Result<bool> {
        self.inner
            .drop_entity(root_block_id, source_id, target_id)
            .await
    }

    /// Override for the atomic-editor PBT path (`apply_press_key`,
    /// `apply_type_chars`, `apply_move_cursor`, `apply_delete_backward`,
    /// `apply_blur`, `apply_focus_editable_text` in
    /// `crates/holon-integration-tests/src/pbt/sut.rs`). These call
    /// `send_raw_keystroke(keystroke, &modifiers)` — the trait default
    /// is `unimplemented!`. We translate the keystroke string to an
    /// `InputEvent` and push through the same input channel.
    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        let ev = raw_keystroke_to_input_event(keystroke, modifiers)?;
        self.send_input(ev).await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        self.await_render(deadline).await
    }

    fn dispatches_chords_via_raw_keystroke(&self) -> bool {
        true
    }

    /// TUI tree-aware click — Enter dispatches whatever the focused widget
    /// binds on Enter (`navigation.focus` on a Selectable, edit-mode on a
    /// Block). The bool return mirrors `click_entity` semantics: we can't
    /// synchronously prove which intent fired, so return `false`.
    async fn click_entity_with_tree(
        &self,
        _: &holon_api::EntityUri,
        _: &ReactiveViewModel,
        entity_id: &holon_api::EntityUri,
        region: &str,
    ) -> Result<bool> {
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    // ── Observation verbs ──────────────────────────────────────────────
    //
    // The TUI's medium is its per-frame `RenderRegistry`, shared with the
    // renderer through `geometry` and `last_registry`. Every
    // `SelectableRegion` carries the entity id, painted bounds, baked
    // click intent, displayed text, and the top-level columns slot it
    // lives under. The verbs read that registry — the same source the
    // keyboard handler and `TuiGeometry` consult — so observation matches
    // what the user sees. This parallels the GPUI screen driver (which
    // reads its `BoundsRegistry`) rather than the headless VM-tree walk.

    fn is_widget_visible(&self, entity_id: &holon_api::EntityUri) -> bool {
        self.geometry
            .find_by_entity_id(entity_id.as_str())
            .filter(|info| info.area() > 0.0)
            .is_some()
    }

    fn is_in_region(&self, entity_id: &holon_api::EntityUri, region: holon_api::Region) -> bool {
        self.entities_in_region(region)
            .iter()
            .any(|uri| uri == entity_id)
    }

    /// Entity ids painted under `region`'s top-level columns slot. The
    /// outermost `render_columns` tags each `SelectableRegion` with the
    /// slot index it sits under (left sidebar = 0, main = 1, right sidebar
    /// = 2 in the default layout); we filter the registry by that slot.
    /// Only regions with non-zero painted extent count as on-screen.
    fn entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        let registry = self.last_registry.lock().unwrap();
        registry_entities_in_slot(&registry, region_columns_slot(region))
    }

    /// The TUI redraws the whole screen each frame and has no virtualized
    /// lists — every entity in a region's tree is painted with bounds (see
    /// `scroll_to_entity`). So "reachable by scrolling" equals "currently
    /// in region", mirroring the headless `ReactiveEngineDriver`.
    fn reachable_entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        self.entities_in_region(region)
    }

    async fn scroll_to_entity(&self, _: &holon_api::EntityUri) -> Result<()> {
        // TUI has no virtualized lists today — every rendered entity has
        // bounds via its full-screen redraw. Return Ok so the
        // `wait_for_entity_bounds` scroll RPC is a benign no-op under TUI;
        // a missing entity still surfaces as the polling timeout, which is
        // the right failure signal. Replace with real page-up/down driving
        // if/when TUI ever grows a virtualized list.
        Ok(())
    }

    /// The click intent the TUI baked into the `selectable` at
    /// shadow-build time — the same value Enter dispatches on the focused
    /// row. Read from the registry entry for `entity_id`; `Block` rows and
    /// selectables without an `action:` have `None`.
    fn click_intent_of(
        &self,
        entity_id: &holon_api::EntityUri,
    ) -> Option<holon_frontend::operations::OperationIntent> {
        let registry = self.last_registry.lock().unwrap();
        registry_click_intent(&registry, entity_id.as_str())
    }

    /// The text the TUI actually painted for `entity_id` — read from the
    /// registry's recorded `displayed_text` (the in-edit buffer when
    /// active, else the resolved `content` prop), surfaced through the
    /// geometry adapter. Mirrors the GPUI driver reading
    /// `ElementInfo::displayed_text`.
    fn displayed_text(&self, entity_id: &holon_api::EntityUri) -> Option<String> {
        self.geometry
            .find_by_entity_id(entity_id.as_str())
            .and_then(|info| info.displayed_text.map(|s| s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use holon_api::EntityName;
    use holon_api::Region;

    use super::*;
    use crate::render::SelectableKind;
    use crate::render::SelectableRegion;

    fn region_entry(
        entity_id: &str,
        slot: usize,
        intent: Option<OperationIntent>,
    ) -> SelectableRegion {
        SelectableRegion {
            entity_id: entity_id.to_string(),
            intent,
            kind: SelectableKind::Block,
            region: slot,
            editable: None,
            start_row: 0,
            start_col: 0,
            rows: 1,
            cols: 10,
            widget_type: "render_entity".to_string(),
            displayed_text: None,
        }
    }

    #[test]
    fn region_slot_maps_panels_in_layout_order() {
        assert_eq!(region_columns_slot(Region::LeftSidebar), 0);
        assert_eq!(region_columns_slot(Region::Main), 1);
        assert_eq!(region_columns_slot(Region::RightSidebar), 2);
    }

    #[test]
    fn entities_in_slot_filters_by_region_and_visible_extent() {
        let mut zero_extent = region_entry("block:hidden", 1, None);
        zero_extent.rows = 0;
        let registry = RenderRegistry {
            selectables: vec![
                region_entry("block:sidebar-item", 0, None),
                region_entry("block:main-a", 1, None),
                region_entry("block:main-b", 1, None),
                zero_extent, // same region but unpainted -> excluded
            ],
        };

        let main = registry_entities_in_slot(&registry, 1);
        let ids: Vec<&str> = main.iter().map(|u| u.as_str()).collect();
        assert!(ids.contains(&"block:main-a"));
        assert!(ids.contains(&"block:main-b"));
        assert!(!ids.contains(&"block:hidden"));
        assert_eq!(main.len(), 2);

        let sidebar = registry_entities_in_slot(&registry, 0);
        assert_eq!(sidebar.len(), 1);
        assert_eq!(sidebar[0].as_str(), "block:sidebar-item");
    }

    #[test]
    fn entities_in_slot_drops_non_uri_ids() {
        // Raw row ids without a scheme aren't addressable URIs and are dropped.
        let registry = RenderRegistry {
            selectables: vec![region_entry("not a uri!", 1, None)],
        };
        assert!(registry_entities_in_slot(&registry, 1).is_empty());
    }

    #[test]
    fn click_intent_reads_baked_intent_and_none_for_blocks() {
        let intent = OperationIntent::new(
            EntityName::new("block"),
            "navigation_focus".to_string(),
            std::collections::HashMap::new(),
        );
        let registry = RenderRegistry {
            selectables: vec![
                region_entry("block:nav", 0, Some(intent)),
                region_entry("block:plain", 1, None),
            ],
        };

        let found = registry_click_intent(&registry, "block:nav").expect("intent present");
        assert_eq!(found.op_name, "navigation_focus");
        assert!(registry_click_intent(&registry, "block:plain").is_none());
        assert!(registry_click_intent(&registry, "block:absent").is_none());
    }
}
