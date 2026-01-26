use crate::keybindings::{
    self, BindingMode, KeyKind, KeyMatch, Modifiers, SpecialKey as KbSpecialKey,
};
use crate::render::{render_view_model, EditableTarget, RenderCtx, RenderRegistry};
use crate::stylesheet;
use holon::sync::mutable_text::TextOp;
use holon_api::{EntityName, Value};
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::{FrontendSession, ReactiveViewModel};
use r3bl_tui::{
    col, height, new_style, render_tui_styled_texts_into, row, surface, throws_with_return,
    tui_color, tui_styled_text, tui_styled_texts, App, BoxedSafeApp, CommonResult,
    ComponentRegistryMap, EventPropagation, FlexBoxId, GlobalData, HasFocus, InputEvent, Key,
    KeyPress, LayoutManagement, LengthOps, Pos, RenderOpCommon, RenderOpIRVec, RenderPipeline,
    Size, SpecialKey, SurfaceProps, TerminalWindowMainThreadSignal, ZOrder, SPACER_GLYPH,
};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub enum AppSignal {
    #[default]
    Noop,
}

/// Application state for r3bl framework.
///
/// `current_model` is updated by a background tokio task (subscribing to the
/// engine's `watch_signal`) and read on every render. The Mutex is held only
/// during the swap/clone, so contention is negligible.
///
/// `last_registry` is rewritten on every render with the selectable regions
/// the walk discovered. The keyboard handler reads it to dispatch the
/// focused region's click intent on Enter.
///
/// `focus_index` tracks which selectable is currently keyboard-focused.
/// `usize::MAX` means "no focus yet" — set on first render once the
/// registry is populated.
#[derive(Clone)]
pub struct TuiState {
    pub session: Arc<FrontendSession>,
    pub engine: Arc<ReactiveEngine>,
    pub rt_handle: tokio::runtime::Handle,
    pub status_message: String,
    pub current_model: Arc<Mutex<Arc<ReactiveViewModel>>>,
    pub watch_started: Arc<AtomicBool>,
    pub last_registry: Arc<Mutex<RenderRegistry>>,
    pub focus_index: Arc<AtomicUsize>,
    /// Stable (entity_id, kind) of the focused region. Used to keep focus
    /// on the same row when the registry's index order shifts between
    /// renders. Tracking the kind matters because sidebar selectables and
    /// main-panel blocks can share an entity_id (a sidebar entry for a
    /// doc-block has the same id as that doc's first content row when
    /// the doc is a "leaf" page) — without it, reconcile would jump to
    /// the first occurrence regardless of which region the user was
    /// actually navigating.
    pub focus_pin: Arc<Mutex<Option<(String, crate::render::SelectableKind)>>>,
    /// `Some` while the user is editing a Block's inline `editable_text`.
    /// Activated by Enter on a focused `Block` region; canceled by Esc;
    /// committed by Enter (which dispatches a `set_field` intent).
    /// While active, all keystrokes route into the buffer instead of into
    /// the navigation handler.
    pub edit_state: Arc<Mutex<Option<EditState>>>,
    /// Leader-key pending (Space pressed, waiting for chord key).
    pub leader_pending: Arc<AtomicBool>,
}

/// Inline edit state, wrapped in an `EditorViewModel` (CRDT-backed).
///
/// Local keystrokes commit to the Loro CRDT via `vm.apply_local()`.
/// There is no "save" action — changes are live. Enter / Alt+s trigger
/// structural operations (split/indent). Esc cancels edit mode but does
/// NOT undo local changes (already committed to CRDT).
///
/// `cursor` is a byte offset into the VM's current text snapshot.
pub struct EditState {
    pub block_id: String,
    pub field: String,
    pub vm: EditorViewModel,
    pub cursor: usize,
}

/// Sentinel for "no selectable focused yet" stored in `focus_index`.
pub const NO_FOCUS: usize = usize::MAX;

impl std::fmt::Debug for TuiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiState")
            .field("status_message", &self.status_message)
            .finish()
    }
}

impl Default for TuiState {
    fn default() -> Self {
        panic!("TuiState::default() should not be called — use TuiState::new()")
    }
}

impl std::fmt::Display for TuiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TuiState")
    }
}

impl r3bl_tui::HasEditorBuffers for TuiState {
    fn get_mut_editor_buffer(&mut self, _id: FlexBoxId) -> Option<&mut r3bl_tui::EditorBuffer> {
        None
    }
    fn insert_editor_buffer(&mut self, _id: FlexBoxId, _buffer: r3bl_tui::EditorBuffer) {}
    fn contains_editor_buffer(&self, _id: FlexBoxId) -> bool {
        false
    }
}

impl r3bl_tui::HasDialogBuffers for TuiState {
    fn get_mut_dialog_buffer(&mut self, _id: FlexBoxId) -> Option<&mut r3bl_tui::DialogBuffer> {
        None
    }
}

pub struct AppMain {
    _phantom: PhantomData<(TuiState, AppSignal)>,
}

impl Default for AppMain {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl AppMain {
    pub fn new_boxed() -> BoxedSafeApp<TuiState, AppSignal> {
        Box::new(Self::default())
    }
}

impl App for AppMain {
    type S = TuiState;
    type AS = AppSignal;

    fn app_init(
        &mut self,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) {
    }

    fn app_handle_input_event(
        &mut self,
        input_event: InputEvent,
        global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        throws_with_return!({
            // Handle Ctrl+Q exit
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('q'),
                mask,
            }) = input_event
            {
                if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed {
                    return Ok(EventPropagation::Propagate);
                }
            }

            // While editing, route every other key into the edit buffer.
            // Esc cancels, Enter commits, Backspace/Left/Right/Home/End/Delete
            // move/edit, character keys insert. Ctrl+q (handled above) and
            // r3bl's exit_keys still take precedence so the user is never
            // trapped.
            if global_data.state.edit_state.lock().unwrap().is_some() {
                return Ok(handle_edit_input(&mut global_data.state, input_event));
            }

            // Handle Ctrl+R sync
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('r'),
                mask,
            }) = input_event
            {
                if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed {
                    let engine = global_data.state.session.engine().clone();
                    tracing::info!("[TUI] Sync triggered (Ctrl+r)");
                    tokio::spawn(async move {
                        let params = std::collections::HashMap::new();
                        match engine
                            .execute_operation(&EntityName::new("*"), "sync", params)
                            .await
                        {
                            Ok(_) => tracing::info!("[TUI] Sync completed"),
                            Err(e) => tracing::error!("[TUI] Sync failed: {}", e),
                        }
                    });
                    global_data.state.status_message = "Syncing...".to_string();
                    return Ok(EventPropagation::ConsumedRender);
                }
            }

            // Ctrl+T: cycle task state on the focused Block. Mirrors GPUI's
            // Cmd+Enter on a focused editor — picks the next state in the
            // configured cycle (TODO → DOING → DONE → unset, etc.). r3bl
            // doesn't expose a Cmd/Super modifier so we use Ctrl+T.
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('t'),
                mask,
            }) = input_event
            {
                if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed {
                    let dispatched = cycle_task_state_on_focused(&global_data.state);
                    global_data.state.status_message = if dispatched {
                        "Cycled task state".to_string()
                    } else {
                        "Nothing to cycle".to_string()
                    };
                    return Ok(EventPropagation::ConsumedRender);
                }
            }

            // Alt+i / Alt+o: indent / outdent the focused Block. GPUI binds
            // Tab/Shift+Tab to these; TUI Tab is taken by region-switch, and
            // Alt+arrow encodings vary across terminal emulators (some send
            // ESC[1;3C for Alt+Right, others split into ESC + Right and lose
            // the modifier). Alt+letter is reliably round-tripped by every
            // crossterm-supported terminal. `i` for "indent", `o` for "out".
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('i'),
                mask,
            }) = input_event
            {
                if mask.alt_key_state == r3bl_tui::KeyState::Pressed {
                    let dispatched = dispatch_block_op_on_focused(&global_data.state, "indent");
                    global_data.state.status_message = if dispatched {
                        "Indented".to_string()
                    } else {
                        "Nothing to indent".to_string()
                    };
                    return Ok(EventPropagation::ConsumedRender);
                }
            }
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('o'),
                mask,
            }) = input_event
            {
                if mask.alt_key_state == r3bl_tui::KeyState::Pressed {
                    let dispatched = dispatch_block_op_on_focused(&global_data.state, "outdent");
                    global_data.state.status_message = if dispatched {
                        "Outdented".to_string()
                    } else {
                        "Nothing to outdent".to_string()
                    };
                    return Ok(EventPropagation::ConsumedRender);
                }
            }

            // ── Leader-key: Space prefixes command chords ──────────
            // When leader is pending, the next key disposition decides the
            // operation. Only active in navigation mode (not while editing).
            if global_data.state.edit_state.lock().unwrap().is_none() {
                let leader_active = global_data.state.leader_pending.load(Ordering::Acquire);

                if leader_active {
                    global_data
                        .state
                        .leader_pending
                        .store(false, Ordering::Release);
                    global_data.state.status_message = "Ready".to_string();

                    if let Some(km) = key_match_from_input(&input_event, /*leader=*/ true) {
                        if let Some(action) = keybindings::global()
                            .action_for(BindingMode::Navigation, &km)
                            .map(str::to_string)
                        {
                            run_navigation_action(&action, &mut global_data.state);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                    }
                    // Leader consumed — ignore the key if not mapped.
                    return Ok(EventPropagation::ConsumedRender);
                }

                // Activate leader on Space (only in navigation mode).
                if let InputEvent::Keyboard(KeyPress::Plain {
                    key: Key::Character(' '),
                }) = input_event
                {
                    global_data
                        .state
                        .leader_pending
                        .store(true, Ordering::Release);
                    global_data.state.status_message =
                        "Leader · ↑↓ move block · ←→ indent/outdent · ⏎ edit · x toggle"
                            .to_string();
                    return Ok(EventPropagation::ConsumedRender);
                }
            }

            // Up / Down: walk selectables WITHIN the active region (sidebar /
            // main / drawer). Tab / Shift-Tab hops BETWEEN regions, so Tab
            // from the last sidebar entry lands on the first main-panel block
            // instead of cycling. Shift-Tab arrives as either
            // Plain { BackTab } (some terminals) or WithModifiers (others) —
            // accept both forms.
            if let InputEvent::Keyboard(kp) = input_event {
                let (key, shift) = match kp {
                    KeyPress::Plain { key } => (key, false),
                    KeyPress::WithModifiers { key, mask } => {
                        (key, mask.shift_key_state == r3bl_tui::KeyState::Pressed)
                    }
                };
                if let Key::SpecialKey(sk) = key {
                    match sk {
                        SpecialKey::Down => {
                            advance_focus(&global_data.state, 1);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                        SpecialKey::Up => {
                            advance_focus(&global_data.state, -1);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                        SpecialKey::Tab if !shift => {
                            switch_region(&global_data.state, 1);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                        SpecialKey::Tab if shift => {
                            switch_region(&global_data.state, -1);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                        SpecialKey::BackTab => {
                            switch_region(&global_data.state, -1);
                            return Ok(EventPropagation::ConsumedRender);
                        }
                        _ => {}
                    }
                }
            }

            // Enter: on a `Selectable` region, dispatch the click intent
            // (typically navigation_focus). On a `Block` region with an
            // inline editable_text, enter edit mode (the very next render
            // shows the buffer + cursor).
            if let InputEvent::Keyboard(KeyPress::Plain {
                key: Key::SpecialKey(SpecialKey::Enter),
            }) = input_event
            {
                let action = enter_pressed(&mut global_data.state);
                global_data.state.status_message = action.into();
                return Ok(EventPropagation::ConsumedRender);
            }

            EventPropagation::Propagate
        });
    }

    fn app_handle_signal(
        &mut self,
        _action: &AppSignal,
        _global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        throws_with_return!({ EventPropagation::ConsumedRender });
    }

    fn app_render(
        &mut self,
        global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<RenderPipeline> {
        throws_with_return!({
            // Spawn the watch task on the very first render — needs the
            // main_thread_channel_sender from GlobalData, which isn't accessible
            // before the event loop starts.
            ensure_watch_task_started(global_data);

            let window_size = global_data.window_size;
            let state = &global_data.state;

            let model: Arc<ReactiveViewModel> = state.current_model.lock().unwrap().clone();

            let mut surface = {
                let mut it = surface!(stylesheet: stylesheet::create_stylesheet()?);

                it.surface_start(SurfaceProps {
                    pos: row(0) + col(0),
                    size: window_size.col_width + (window_size.row_height - height(2)),
                })?;

                // Title bar
                {
                    let mut title_ops = RenderOpIRVec::new();
                    title_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(2), row(0))));
                    let title_texts = tui_styled_texts! {
                        tui_styled_text! {
                            @style: new_style!(bold color_fg: {tui_color!(hex "#00AAFF")}),
                            @text: "Holon (R3BL TUI)"
                        },
                    };
                    render_tui_styled_texts_into(&title_texts, &mut title_ops);
                    it.render_pipeline.push(ZOrder::Normal, title_ops);
                }

                // Content area — walk the reactive view model.
                {
                    let mut content_ops = RenderOpIRVec::new();
                    let max_width = window_size.col_width.as_usize().saturating_sub(2);
                    let mut registry = RenderRegistry::default();
                    let focus_index = match state.focus_index.load(Ordering::Acquire) {
                        NO_FOCUS => None,
                        i => Some(i),
                    };
                    // Snapshot edit_state under its lock so the view borrow
                    // stays valid for the duration of the render walk.
                    let edit_guard = state.edit_state.lock().unwrap();
                    let edit_view = edit_guard.as_ref().map(|e| crate::render::EditView {
                        block_id: e.block_id.clone(),
                        field: e.field.clone(),
                        buffer: e.vm.current_text().unwrap_or_default(),
                        cursor: e.cursor,
                    });
                    let mut ctx = RenderCtx::new(&state.engine, &mut registry, focus_index)
                        .with_edit(edit_view);
                    render_view_model(
                        model.as_ref(),
                        &mut ctx,
                        &mut content_ops,
                        2, // start_row (after title)
                        2, // start_col
                        max_width,
                    );
                    drop(edit_guard);
                    it.render_pipeline.push(ZOrder::Normal, content_ops);

                    // Re-anchor focus to the same entity_id across renders so
                    // the user's selection stays put when the registry's
                    // index order shifts (new sidebar item, collection
                    // reorder). Initialise focus on the first selectable
                    // discovered if there isn't one yet.
                    reconcile_focus(state, &registry);
                    *state.last_registry.lock().unwrap() = registry;
                }

                it.surface_end()?;
                it
            };

            // Status bar
            render_status_bar(
                &mut surface.render_pipeline,
                window_size,
                &state.status_message,
            );

            surface.render_pipeline
        });
    }
}

/// Subscribe to the engine's reactive signal once. Each emission updates the
/// state's model holder and dispatches a `Render` request through r3bl's main
/// thread channel so the view refreshes immediately on CDC.
///
/// The structural-only `watch_signal` doesn't fire on data-only changes, so a
/// second short-period ticker drives renders at ~10 Hz to pick up reactive
/// `data`/`props`/collection updates inside the existing tree.
fn ensure_watch_task_started(global_data: &mut GlobalData<TuiState, AppSignal>) {
    let started = global_data.state.watch_started.clone();
    if started.swap(true, Ordering::SeqCst) {
        return;
    }

    let model_holder = global_data.state.current_model.clone();
    let engine = global_data.state.engine.clone();
    let main_sender = global_data.main_thread_channel_sender.clone();
    let rt_handle = global_data.state.rt_handle.clone();

    rt_handle.spawn(async move {
        use futures::StreamExt;
        let root_uri = holon_api::root_layout_block_uri();
        let mut stream = engine.watch(&root_uri);
        while let Some(rvm) = stream.next().await {
            {
                let mut slot = model_holder.lock().unwrap();
                *slot = Arc::new(rvm);
            }
            let _ = main_sender
                .send(TerminalWindowMainThreadSignal::Render(None))
                .await;
        }
        tracing::warn!("[TUI] Reactive watch stream ended");
    });

    // Periodic re-render to pick up data-only updates inside the live tree.
    let main_sender_tick = global_data.main_thread_channel_sender.clone();
    rt_handle.spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(100));
        loop {
            ticker.tick().await;
            if main_sender_tick
                .send(TerminalWindowMainThreadSignal::Render(None))
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

/// Cycle the focus by `delta` (1 = forward, -1 = backward) WITHIN the current
/// region. Up/Down should feel like walking through the items of the active
/// panel, not jumping out into the sidebar mid-flight. Tab/Shift-Tab is the
/// shortcut that hops between regions (see [`switch_region`]).
///
/// No-op when the registry is empty. When no selectable is focused yet, picks
/// the first selectable in the lowest region.
fn advance_focus(state: &TuiState, delta: i32) {
    let registry = state.last_registry.lock().unwrap();
    if registry.selectables.is_empty() {
        return;
    }
    let active_region = active_region_of(&registry, state.focus_index.load(Ordering::Acquire));
    let region_indices: Vec<usize> = registry
        .selectables
        .iter()
        .enumerate()
        .filter_map(|(i, sel)| (sel.region == active_region).then_some(i))
        .collect();
    if region_indices.is_empty() {
        return;
    }

    let current = state.focus_index.load(Ordering::Acquire);
    let next = if current == NO_FOCUS {
        if delta >= 0 {
            region_indices[0]
        } else {
            *region_indices.last().unwrap()
        }
    } else {
        let cur_pos = region_indices
            .iter()
            .position(|&i| i == current)
            .unwrap_or(0);
        let n = region_indices.len();
        let next_pos = if delta >= 0 {
            (cur_pos + 1) % n
        } else {
            (cur_pos + n - 1) % n
        };
        region_indices[next_pos]
    };
    state.focus_index.store(next, Ordering::Release);
    let new_pin = registry
        .selectables
        .get(next)
        .map(|sel| (sel.entity_id.clone(), sel.kind));
    *state.focus_pin.lock().unwrap() = new_pin;
}

/// Hop focus to the first selectable of the next/previous top-level region
/// (sidebar → main → drawer → wraps). Bound to Tab / Shift-Tab. No-op when
/// the registry has only one region's worth of selectables.
fn switch_region(state: &TuiState, delta: i32) {
    let registry = state.last_registry.lock().unwrap();
    if registry.selectables.is_empty() {
        return;
    }
    let mut regions: Vec<usize> = registry.selectables.iter().map(|s| s.region).collect();
    regions.sort_unstable();
    regions.dedup();
    if regions.len() <= 1 {
        return;
    }
    let active_region = active_region_of(&registry, state.focus_index.load(Ordering::Acquire));
    let cur_pos = regions
        .iter()
        .position(|&r| r == active_region)
        .unwrap_or(0);
    let n = regions.len();
    let next_pos = if delta >= 0 {
        (cur_pos + 1) % n
    } else {
        (cur_pos + n - 1) % n
    };
    let target_region = regions[next_pos];
    let Some((idx, sel)) = registry
        .selectables
        .iter()
        .enumerate()
        .find(|(_, s)| s.region == target_region)
    else {
        return;
    };
    state.focus_index.store(idx, Ordering::Release);
    *state.focus_pin.lock().unwrap() = Some((sel.entity_id.clone(), sel.kind));
}

/// Dispatch `block.cycle_task_state` on the currently focused `Block`.
/// Thin wrapper around [`dispatch_block_op_on_focused`].
fn cycle_task_state_on_focused(state: &TuiState) -> bool {
    dispatch_block_op_on_focused(state, "cycle_task_state")
}

/// Translate an r3bl `InputEvent` into a `keybindings::KeyMatch` for
/// table lookup. Returns `None` for non-keyboard events or unsupported
/// keys (e.g. `FunctionKey`); the caller treats that as "not bound".
fn key_match_from_input(ev: &InputEvent, leader: bool) -> Option<KeyMatch> {
    let keypress = match ev {
        InputEvent::Keyboard(kp) => kp,
        _ => return None,
    };
    let (key, mut modifiers) = match keypress {
        KeyPress::Plain { key } => (key, Modifiers::default()),
        KeyPress::WithModifiers { key, mask } => {
            let mods = Modifiers {
                ctrl: mask.ctrl_key_state == r3bl_tui::KeyState::Pressed,
                alt: mask.alt_key_state == r3bl_tui::KeyState::Pressed,
                shift: mask.shift_key_state == r3bl_tui::KeyState::Pressed,
                leader: false,
            };
            (key, mods)
        }
    };
    modifiers.leader = leader;
    let kind = match key {
        Key::Character(c) => KeyKind::Char(*c),
        Key::SpecialKey(SpecialKey::Up) => KeyKind::Special(KbSpecialKey::Up),
        Key::SpecialKey(SpecialKey::Down) => KeyKind::Special(KbSpecialKey::Down),
        Key::SpecialKey(SpecialKey::Left) => KeyKind::Special(KbSpecialKey::Left),
        Key::SpecialKey(SpecialKey::Right) => KeyKind::Special(KbSpecialKey::Right),
        Key::SpecialKey(SpecialKey::Enter) => KeyKind::Special(KbSpecialKey::Enter),
        Key::SpecialKey(SpecialKey::Tab) => KeyKind::Special(KbSpecialKey::Tab),
        Key::SpecialKey(SpecialKey::Esc) => KeyKind::Special(KbSpecialKey::Esc),
        Key::SpecialKey(SpecialKey::Backspace) => KeyKind::Special(KbSpecialKey::Backspace),
        _ => return None,
    };
    Some(KeyMatch {
        key: kind,
        modifiers,
    })
}

/// Dispatch a YAML-resolved navigation action against the focused
/// state. Updates `status_message` for the user. Action names match
/// the `action:` field of `keybindings.yaml` entries.
fn run_navigation_action(action: &str, state: &mut TuiState) {
    match action {
        "move_up" => {
            let ok = dispatch_block_op_on_focused(state, "move_up");
            state.status_message = if ok {
                "Moved up".into()
            } else {
                "Cannot move up".into()
            };
        }
        "move_down" => {
            let ok = dispatch_block_op_on_focused(state, "move_down");
            state.status_message = if ok {
                "Moved down".into()
            } else {
                "Cannot move down".into()
            };
        }
        "indent" => {
            let ok = dispatch_block_op_on_focused(state, "indent");
            state.status_message = if ok {
                "Indented".into()
            } else {
                "Cannot indent".into()
            };
        }
        "outdent" => {
            let ok = dispatch_block_op_on_focused(state, "outdent");
            state.status_message = if ok {
                "Outdented".into()
            } else {
                "Cannot outdent".into()
            };
        }
        "cycle_task_state" => {
            let ok = dispatch_block_op_on_focused(state, "cycle_task_state");
            state.status_message = if ok {
                "Cycled task state".into()
            } else {
                "Nothing to cycle".into()
            };
        }
        "start_editing" => {
            let result = enter_pressed(state);
            state.status_message = result.into();
        }
        "go_home" => dispatch_navigation_op(state, "go_home", "Home"),
        "go_back" => dispatch_navigation_op(state, "go_back", "Back"),
        "go_forward" => dispatch_navigation_op(state, "go_forward", "Forward"),
        other => {
            state.status_message = format!("Unknown action '{other}' from keybindings.yaml");
        }
    }
}

/// Dispatch a `navigation.<op>` intent for `region: "main"` through
/// the engine. The engine's `maybe_mirror_navigation_focus` keeps
/// `ui_state.focused_block` in sync; the SQL backend handles
/// navigation_history mutation.
fn dispatch_navigation_op(state: &mut TuiState, op_name: &str, label: &str) {
    let mut params = std::collections::HashMap::new();
    params.insert("region".to_string(), Value::String("main".to_string()));
    let intent = OperationIntent::new(
        EntityName::Named("navigation".to_string()),
        op_name.to_string(),
        params,
    );
    state.engine.dispatch_intent(intent);
    state.status_message = label.to_string();
}

/// Dispatch a `block.<op_name>` intent (only an `id` param — no `value`,
/// `position`, etc.) against the currently focused `Block` region's entity
/// id. Used by the Ctrl+T / Alt+Right / Alt+Left shortcuts. Returns `true`
/// when an intent fired, `false` for any of: no focus, focus is on a
/// `Selectable` rather than a `Block`, focus_index out of bounds.
fn dispatch_block_op_on_focused(state: &TuiState, op_name: &str) -> bool {
    use crate::render::SelectableKind;

    let registry = state.last_registry.lock().unwrap();
    let idx = state.focus_index.load(Ordering::Acquire);
    if idx == NO_FOCUS {
        if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
            eprintln!("[chord] {op_name}: no focus");
        }
        return false;
    }
    let Some(region) = registry.selectables.get(idx) else {
        if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
            eprintln!("[chord] {op_name}: focus_index {idx} out of bounds");
        }
        return false;
    };
    if region.kind != SelectableKind::Block {
        if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
            eprintln!(
                "[chord] {op_name}: focused region is {:?}, not Block",
                region.kind
            );
        }
        return false;
    }
    let block_id = region.entity_id.clone();
    drop(registry);

    if std::env::var("HOLON_DEBUG_CHORD").is_ok() {
        eprintln!("[chord] {op_name}: dispatching on block {block_id}");
    }

    let engine = state.engine.clone();
    let op = op_name.to_string();
    state.rt_handle.spawn(async move {
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), Value::String(block_id));
        let intent =
            OperationIntent::new(EntityName::Named("block".to_string()), op.clone(), params);
        if let Err(e) = engine.dispatch_intent_sync(intent).await {
            tracing::error!("{op} failed: {e}");
        }
    });
    true
}

/// Region the currently-focused selectable lives in, or 0 when no focus.
fn active_region_of(registry: &RenderRegistry, focus_idx: usize) -> usize {
    if focus_idx == NO_FOCUS {
        return registry.selectables.first().map(|s| s.region).unwrap_or(0);
    }
    registry
        .selectables
        .get(focus_idx)
        .map(|s| s.region)
        .unwrap_or(0)
}

/// Re-pin focus to the same `entity_id` across renders. If the focused
/// entity_id from the previous render still appears in the new registry,
/// update `focus_index` to its new position. Otherwise fall back to the
/// first `Block` region if any — so initial load and post-`Enter`
/// async re-renders (where the previously-focused block no longer exists
/// because the doc just changed) land the cursor on the first row of the
/// active doc. Final fallback: index 0 (or NO_FOCUS if empty).
fn reconcile_focus(state: &TuiState, registry: &RenderRegistry) {
    use crate::render::SelectableKind;

    let len = registry.selectables.len();
    if len == 0 {
        state.focus_index.store(NO_FOCUS, Ordering::Release);
        *state.focus_pin.lock().unwrap() = None;
        return;
    }

    let pinned = state.focus_pin.lock().unwrap().clone();
    if let Some((pinned_id, pinned_kind)) = &pinned {
        if let Some((idx, _)) = registry
            .selectables
            .iter()
            .enumerate()
            .find(|(_, sel)| sel.kind == *pinned_kind && &sel.entity_id == pinned_id)
        {
            state.focus_index.store(idx, Ordering::Release);
            return;
        }
    }

    // Pinned entity gone (or first render) — prefer first Block over
    // first overall, so a freshly-opened doc lands on its first row
    // rather than back at the first sidebar entry. Falls through to
    // idx 0 only when no Block exists.
    let fallback_idx = registry
        .selectables
        .iter()
        .position(|sel| sel.kind == SelectableKind::Block)
        .unwrap_or(0);
    state.focus_index.store(fallback_idx, Ordering::Release);
    let region = &registry.selectables[fallback_idx];
    *state.focus_pin.lock().unwrap() = Some((region.entity_id.clone(), region.kind));
}

/// What just happened when the user pressed Enter. The discriminant doubles
/// as the status-bar message — see [`EnterAction::into`].
enum EnterAction {
    Activated,
    EnteredEdit,
    Nothing,
}

impl From<EnterAction> for String {
    fn from(a: EnterAction) -> Self {
        match a {
            EnterAction::Activated => "Activated".to_string(),
            EnterAction::EnteredEdit => "Editing — Esc to cancel, Enter to save".to_string(),
            EnterAction::Nothing => "Nothing focused".to_string(),
        }
    }
}

/// Decision computed from a render registry + focus index, with no engine
/// or async dependencies. Lets `enter_pressed` be unit-tested without
/// constructing a full `TuiState`.
#[derive(Debug)]
enum EnterDecision {
    Nothing,
    DispatchSelectable(OperationIntent),
    EnterEdit(EditableTarget),
}

/// Pure decision: given the focused region's shape, what should
/// `enter_pressed` do? See `decide_enter_action_*` tests.
fn decide_enter_action(registry: &RenderRegistry, idx: usize) -> EnterDecision {
    use crate::render::SelectableKind;

    if idx == NO_FOCUS {
        return EnterDecision::Nothing;
    }
    let Some(region) = registry.selectables.get(idx) else {
        return EnterDecision::Nothing;
    };
    if region.kind == SelectableKind::Selectable {
        return match region.intent.clone() {
            Some(intent) => EnterDecision::DispatchSelectable(intent),
            None => EnterDecision::Nothing,
        };
    }
    match region.editable.clone() {
        Some(target) => EnterDecision::EnterEdit(target),
        None => EnterDecision::Nothing,
    }
}

/// Build the `navigation.editor_focus` intent that mirrors GPUI's
/// `render_entity` click handler. Extracted so unit tests can verify the
/// intent's entity/op/params shape without booting the engine.
fn editor_focus_intent_for(block_id: &str) -> OperationIntent {
    let mut params = std::collections::HashMap::new();
    params.insert("block_id".to_string(), Value::String(block_id.to_string()));
    OperationIntent::new(
        EntityName::Named("navigation".to_string()),
        "editor_focus".to_string(),
        params,
    )
}

/// Called when the user presses Enter outside edit mode. Three branches:
///
/// 1. Focused region is a `Selectable` (sidebar entry): dispatch its click
///    intent — same path GPUI's mouse-down uses. Drops the focus pin so
///    `reconcile_focus` lands on the new doc's first Block on the next
///    render.
/// 2. Focused region is a `Block` with an inline `editable_text`: seed
///    `edit_state` from the editable's current content. The next render
///    will paint the buffer + cursor.
/// 3. Nothing focused or focused Block has no editable target: no-op.
fn enter_pressed(state: &mut TuiState) -> EnterAction {
    let decision = {
        let registry = state.last_registry.lock().unwrap();
        let idx = state.focus_index.load(Ordering::Acquire);
        decide_enter_action(&registry, idx)
    };
    let target = match decision {
        EnterDecision::Nothing => return EnterAction::Nothing,
        EnterDecision::DispatchSelectable(intent) => {
            state.engine.dispatch_intent(intent);
            *state.focus_pin.lock().unwrap() = None;
            return EnterAction::Activated;
        }
        EnterDecision::EnterEdit(target) => target,
    };

    // Mirror the GPUI render_entity click handler: dispatch
    // `navigation.editor_focus` so the engine's `focused_block`
    // tracks the block being edited. Without this, chord-resolution
    // (`bubble_input_oneshot`) and any consumer of `ui_state.focused_block`
    // see stale focus, breaking keybindings on the freshly-opened editor.
    state
        .engine
        .dispatch_intent(editor_focus_intent_for(&target.block_id));

    let mt = match state.engine.editable_text(&target.block_id, &target.field) {
        Ok(mt) => mt,
        Err(e) => {
            state.status_message = format!("editable text unavailable: {e}");
            return EnterAction::Nothing;
        }
    };
    let mut vm = EditorViewModel::new(
        Vec::new(),
        Vec::new(),
        std::collections::HashMap::new(),
        target.field.clone(),
        mt.current(),
    );
    vm.attach_mutable_text(mt);
    let cursor = vm.current_text().unwrap_or_default().len();
    // Snapshot a remote-delta stream BEFORE moving the VM into EditState.
    // The stream is an independent broadcast subscription, so the VM can
    // still be moved.
    let remote_stream = vm.remote_deltas();
    let block_id_for_task = target.block_id.clone();
    let field_for_task = target.field.clone();
    *state.edit_state.lock().unwrap() = Some(EditState {
        block_id: target.block_id,
        field: target.field,
        vm,
        cursor,
    });
    // Spawn the remote-delta consumer. It exits when the user leaves
    // edit mode or switches to a different block/field. Redraws are
    // picked up by the existing 100 ms periodic ticker in
    // ensure_watch_task_started, so we only need to keep cursor + text
    // in sync here.
    let edit_state_arc = state.edit_state.clone();
    state.rt_handle.spawn(async move {
        let Some(mut stream) = remote_stream else {
            return;
        };
        use futures::StreamExt;
        while stream.next().await.is_some() {
            let mut guard = edit_state_arc.lock().unwrap();
            match guard.as_mut() {
                Some(es) if es.block_id == block_id_for_task && es.field == field_for_task => {
                    let new_text = es.vm.current_text().unwrap_or_default();
                    if es.cursor > new_text.len() {
                        es.cursor = new_text.len();
                    }
                }
                _ => break,
            }
        }
    });
    EnterAction::EnteredEdit
}

/// Edit-mode key handler. Routes keystrokes into MutableText (CRDT-backed).
/// Changes are committed to Loro immediately — no flush/save step needed.
/// Enter / Alt+s / Backspace-at-0 trigger structural operations without a
/// prior set_field because the CRDT→SQL CDC pipeline already propagates
/// every keystroke.
fn handle_edit_input(state: &mut TuiState, input_event: InputEvent) -> EventPropagation {
    let InputEvent::Keyboard(kp) = input_event else {
        return EventPropagation::ConsumedRender;
    };
    let key = match kp {
        KeyPress::Plain { key } => key,
        KeyPress::WithModifiers { key, .. } => key,
    };

    let alt = matches!(
        kp,
        KeyPress::WithModifiers { mask, .. }
            if mask.alt_key_state == r3bl_tui::KeyState::Pressed
    );
    let ctrl_key = matches!(
        kp,
        KeyPress::WithModifiers { mask, .. }
            if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed
    );

    let mut edit = state.edit_state.lock().unwrap();
    let Some(edit_state) = edit.as_mut() else {
        return EventPropagation::ConsumedRender;
    };

    let current = edit_state.vm.current_text().unwrap_or_default();

    match key {
        Key::SpecialKey(SpecialKey::Esc) => {
            *edit = None;
            drop(edit);
            state.status_message = "Edit exited".to_string();
        }
        Key::Character('x') if ctrl_key => {
            // Ctrl+x: split at cursor (Vim-style).
            // With MutableText the content is already committed to CRDT,
            // so no pre-flush is needed — just dispatch.
            let block_id = edit_state.block_id.clone();
            let cursor = edit_state.cursor as i64;
            let engine = state.engine.clone();
            let rt = state.rt_handle.clone();
            *edit = None;
            drop(edit);
            rt.spawn(async move {
                let mut params = std::collections::HashMap::new();
                params.insert("id".to_string(), Value::String(block_id));
                params.insert("position".to_string(), Value::Integer(cursor));
                let split = OperationIntent::new(
                    EntityName::Named("block".to_string()),
                    "split_block".to_string(),
                    params,
                );
                if let Err(e) = engine.dispatch_intent_sync(split).await {
                    tracing::error!("split_block failed: {e}");
                }
            });
            state.status_message = "Split block".to_string();
        }
        Key::SpecialKey(SpecialKey::Enter) => {
            *edit = None;
            drop(edit);
            state.status_message = "Edit exited".to_string();
        }
        Key::SpecialKey(SpecialKey::Backspace) if edit_state.cursor == 0 => {
            // Backspace at column 0: join with previous sibling.
            let block_id = edit_state.block_id.clone();
            let engine = state.engine.clone();
            let rt = state.rt_handle.clone();
            *edit = None;
            drop(edit);
            rt.spawn(async move {
                let mut params = std::collections::HashMap::new();
                params.insert("id".to_string(), Value::String(block_id));
                params.insert("position".to_string(), Value::Integer(0));
                let join = OperationIntent::new(
                    EntityName::Named("block".to_string()),
                    "join_block".to_string(),
                    params,
                );
                if let Err(e) = engine.dispatch_intent_sync(join).await {
                    tracing::error!("join_block failed: {e}");
                }
            });
            state.status_message = "Join block".to_string();
        }
        Key::SpecialKey(SpecialKey::Backspace) => {
            if edit_state.cursor > 0 {
                let prev = prev_char_boundary(&current, edit_state.cursor);
                let len = edit_state.cursor - prev;
                if let Err(e) = edit_state.vm.apply_local(TextOp::Delete {
                    pos_codepoint: prev,
                    len_codepoint: len,
                }) {
                    tracing::error!("MutableText delete failed: {e}");
                }
                edit_state.cursor = prev;
            }
        }
        Key::SpecialKey(SpecialKey::Delete) => {
            if edit_state.cursor < current.len() {
                let next = next_char_boundary(&current, edit_state.cursor);
                let len = next - edit_state.cursor;
                if let Err(e) = edit_state.vm.apply_local(TextOp::Delete {
                    pos_codepoint: edit_state.cursor,
                    len_codepoint: len,
                }) {
                    tracing::error!("MutableText delete failed: {e}");
                }
            }
        }
        Key::SpecialKey(SpecialKey::Left) => {
            if edit_state.cursor > 0 {
                edit_state.cursor = prev_char_boundary(&current, edit_state.cursor);
            }
        }
        Key::SpecialKey(SpecialKey::Right) => {
            if edit_state.cursor < current.len() {
                edit_state.cursor = next_char_boundary(&current, edit_state.cursor);
            }
        }
        Key::SpecialKey(SpecialKey::Home) => {
            edit_state.cursor = 0;
        }
        Key::SpecialKey(SpecialKey::End) => {
            edit_state.cursor = current.len();
        }
        Key::Character(c) => {
            // Filter out non-printable control characters (e.g. ESC arrives
            // as both SpecialKey::Esc and possibly a raw control char on
            // some terminals; let SpecialKey::Esc handle it). Also drop
            // Alt-modified characters — the only Alt+letter we use in edit
            // mode is `Alt+s` (split), handled above; anything else with
            // Alt would be either a future shortcut or a typo, never
            // intended literal text.
            if !c.is_control() && !alt {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                if let Err(e) = edit_state.vm.apply_local(TextOp::Insert {
                    pos_codepoint: edit_state.cursor,
                    text: s.to_string(),
                }) {
                    tracing::error!("MutableText insert failed: {e}");
                }
                edit_state.cursor += s.len();
            }
        }
        _ => {}
    }
    EventPropagation::ConsumedRender
}

/// Find the byte offset of the char boundary strictly before `from` in `s`.
/// Returns 0 if the prefix is empty. Walks UTF-8 backwards using the
/// `is_char_boundary` predicate so we don't slice into the middle of a
/// multibyte sequence.
fn prev_char_boundary(s: &str, from: usize) -> usize {
    let mut i = from.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Find the byte offset of the char boundary strictly after `from` in `s`.
/// Returns `s.len()` if `from` is already at the end.
fn next_char_boundary(s: &str, from: usize) -> usize {
    let mut i = from + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i.min(s.len())
}

fn render_status_bar(pipeline: &mut RenderPipeline, size: Size, status_msg: &str) {
    let color_bg = tui_color!(hex "#076DEB");
    let color_fg = tui_color!(hex "#E9C940");

    let help_text = format!(
        "^q Exit | ^r Sync | Space · leader (↑↓←→⏎ x) | ↑↓ Move | Enter Edit | {}",
        status_msg
    );

    let styled_texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg:{color_fg} color_bg:{color_bg}),
            @text: &help_text
        },
    };

    let row_idx = row(size.row_height.convert_to_index());

    let mut render_ops = RenderOpIRVec::new();
    render_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(0), row_idx)));
    render_ops += RenderOpCommon::ResetColor;
    render_ops += RenderOpCommon::SetBgColor(color_bg);
    render_ops += r3bl_tui::RenderOpIR::PaintTextWithAttributes(
        SPACER_GLYPH.repeat(size.col_width.as_usize()).into(),
        None,
    );
    render_ops += RenderOpCommon::ResetColor;
    render_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(2), row_idx)));
    render_tui_styled_texts_into(&styled_texts, &mut render_ops);
    pipeline.push(ZOrder::Normal, render_ops);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{EditableTarget, RenderRegistry, SelectableKind, SelectableRegion};

    fn region_block(entity_id: &str, editable: Option<EditableTarget>) -> SelectableRegion {
        SelectableRegion {
            entity_id: entity_id.to_string(),
            intent: None,
            kind: SelectableKind::Block,
            region: 0,
            editable,
            start_row: 0,
            start_col: 0,
            rows: 1,
            cols: 1,
            widget_type: "live_block".to_string(),
            displayed_text: None,
        }
    }

    fn region_selectable(entity_id: &str, intent: Option<OperationIntent>) -> SelectableRegion {
        SelectableRegion {
            entity_id: entity_id.to_string(),
            intent,
            kind: SelectableKind::Selectable,
            region: 0,
            editable: None,
            start_row: 0,
            start_col: 0,
            rows: 1,
            cols: 1,
            widget_type: "selectable".to_string(),
            displayed_text: None,
        }
    }

    fn editable_for(block_id: &str) -> EditableTarget {
        EditableTarget {
            block_id: block_id.to_string(),
            field: "content".to_string(),
            current_content: "hello".to_string(),
        }
    }

    #[test]
    fn editor_focus_intent_has_navigation_entity_and_block_id_param() {
        let intent = editor_focus_intent_for("block:abc-123");
        assert!(matches!(
            intent.entity_name,
            EntityName::Named(ref n) if n == "navigation"
        ));
        assert_eq!(intent.op_name, "editor_focus");
        match intent.params.get("block_id") {
            Some(Value::String(s)) => assert_eq!(s, "block:abc-123"),
            other => panic!("expected block_id String param, got {other:?}"),
        }
        assert_eq!(
            intent.params.len(),
            1,
            "intent must carry exactly one param (block_id) — extras would silently shadow upstream params"
        );
    }

    #[test]
    fn decide_enter_action_no_focus_returns_nothing() {
        let registry = RenderRegistry::default();
        assert!(matches!(
            decide_enter_action(&registry, NO_FOCUS),
            EnterDecision::Nothing
        ));
    }

    #[test]
    fn decide_enter_action_idx_out_of_bounds_returns_nothing() {
        let registry = RenderRegistry::default();
        assert!(matches!(
            decide_enter_action(&registry, 0),
            EnterDecision::Nothing
        ));
    }

    #[test]
    fn decide_enter_action_block_with_editable_returns_enter_edit() {
        let mut registry = RenderRegistry::default();
        registry
            .selectables
            .push(region_block("block:foo", Some(editable_for("block:foo"))));
        match decide_enter_action(&registry, 0) {
            EnterDecision::EnterEdit(target) => {
                assert_eq!(target.block_id, "block:foo");
                assert_eq!(target.field, "content");
            }
            other => panic!("expected EnterEdit, got {other:?}"),
        }
    }

    #[test]
    fn decide_enter_action_block_without_editable_returns_nothing() {
        let mut registry = RenderRegistry::default();
        registry.selectables.push(region_block("block:foo", None));
        assert!(matches!(
            decide_enter_action(&registry, 0),
            EnterDecision::Nothing
        ));
    }

    #[test]
    fn decide_enter_action_selectable_with_intent_returns_dispatch() {
        let mut registry = RenderRegistry::default();
        let intent = OperationIntent::new(
            EntityName::Named("navigation".to_string()),
            "navigate_focus".to_string(),
            Default::default(),
        );
        registry
            .selectables
            .push(region_selectable("doc:bar", Some(intent.clone())));
        match decide_enter_action(&registry, 0) {
            EnterDecision::DispatchSelectable(got) => {
                assert_eq!(got.op_name, intent.op_name);
            }
            other => panic!("expected DispatchSelectable, got {other:?}"),
        }
    }

    #[test]
    fn decide_enter_action_selectable_without_intent_returns_nothing() {
        let mut registry = RenderRegistry::default();
        registry
            .selectables
            .push(region_selectable("doc:bar", None));
        assert!(matches!(
            decide_enter_action(&registry, 0),
            EnterDecision::Nothing
        ));
    }

    /// Regression for the `enter_pressed` editor_focus dispatch contract:
    /// when entering edit mode on a Block region, an
    /// `editor_focus_intent_for(target.block_id)` must be produced. Without
    /// this, the engine's `focused_block` stays stale → chord-resolution
    /// (`bubble_input_oneshot`) and any consumer of `ui_state.focused_block`
    /// see the wrong block, breaking keybindings on the freshly-opened editor.
    /// (See enter_pressed comment block.)
    #[test]
    fn enter_edit_decision_carries_block_id_for_editor_focus_dispatch() {
        let mut registry = RenderRegistry::default();
        registry.selectables.push(region_block(
            "block:editor-focus-target",
            Some(editable_for("block:editor-focus-target")),
        ));
        let decision = decide_enter_action(&registry, 0);
        let target = match decision {
            EnterDecision::EnterEdit(t) => t,
            other => panic!("expected EnterEdit, got {other:?}"),
        };
        let intent = editor_focus_intent_for(&target.block_id);
        match intent.params.get("block_id") {
            Some(Value::String(s)) => {
                assert_eq!(s, "block:editor-focus-target");
            }
            other => panic!("editor_focus intent missing block_id, got {other:?}"),
        }
    }
}
