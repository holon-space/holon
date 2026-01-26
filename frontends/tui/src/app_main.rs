use crate::render::{render_view_model, EditableTarget, RenderCtx, RenderRegistry};
use crate::stylesheet;
use holon::sync::mutable_text::{MutableText, TextOp};
use holon_api::{EntityName, Value};
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

/// Inline edit state using MutableText (CRDT-backed).
///
/// Local keystrokes commit to the Loro CRDT via `mt.apply_local()`.
/// There is no "save" action — changes are live. Enter / Alt+s trigger
/// structural operations (split/indent). Esc cancels edit mode but does
/// NOT undo local changes (already committed to CRDT).
///
/// `cursor` is a byte offset into the MutableText's current content.
#[derive(Clone)]
pub struct EditState {
    pub block_id: String,
    pub field: String,
    pub mt: MutableText,
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

                    if let InputEvent::Keyboard(KeyPress::Plain { key }) = input_event {
                        match key {
                            Key::SpecialKey(SpecialKey::Up) => {
                                let ok =
                                    dispatch_block_op_on_focused(&global_data.state, "move_up");
                                global_data.state.status_message = if ok {
                                    "Moved up".into()
                                } else {
                                    "Cannot move up".into()
                                };
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            Key::SpecialKey(SpecialKey::Down) => {
                                let ok =
                                    dispatch_block_op_on_focused(&global_data.state, "move_down");
                                global_data.state.status_message = if ok {
                                    "Moved down".into()
                                } else {
                                    "Cannot move down".into()
                                };
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            Key::SpecialKey(SpecialKey::Right) => {
                                let ok = dispatch_block_op_on_focused(&global_data.state, "indent");
                                global_data.state.status_message = if ok {
                                    "Indented".into()
                                } else {
                                    "Cannot indent".into()
                                };
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            Key::SpecialKey(SpecialKey::Left) => {
                                let ok =
                                    dispatch_block_op_on_focused(&global_data.state, "outdent");
                                global_data.state.status_message = if ok {
                                    "Outdented".into()
                                } else {
                                    "Cannot outdent".into()
                                };
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            Key::SpecialKey(SpecialKey::Enter) => {
                                let action = enter_pressed(&mut global_data.state);
                                global_data.state.status_message = action.into();
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            Key::Character('x') => {
                                dispatch_block_op_on_focused(
                                    &global_data.state,
                                    "cycle_task_state",
                                );
                                return Ok(EventPropagation::ConsumedRender);
                            }
                            _ => {}
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
                        buffer: e.mt.current(),
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
        return false;
    }
    let Some(region) = registry.selectables.get(idx) else {
        return false;
    };
    if region.kind != SelectableKind::Block {
        return false;
    }
    let block_id = region.entity_id.clone();
    drop(registry);

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
    use crate::render::SelectableKind;

    let registry = state.last_registry.lock().unwrap();
    let idx = state.focus_index.load(Ordering::Acquire);
    if idx == NO_FOCUS {
        return EnterAction::Nothing;
    }
    let Some(region) = registry.selectables.get(idx) else {
        return EnterAction::Nothing;
    };

    if region.kind == SelectableKind::Selectable {
        let Some(intent) = region.intent.clone() else {
            return EnterAction::Nothing;
        };
        drop(registry);
        state.engine.dispatch_intent(intent);
        *state.focus_pin.lock().unwrap() = None;
        return EnterAction::Activated;
    }

    // Block region — start inline edit if there's an editable_text inside.
    let Some(target): Option<EditableTarget> = region.editable.clone() else {
        return EnterAction::Nothing;
    };
    drop(registry);

    let mt = match state.engine.editable_text(&target.block_id, &target.field) {
        Ok(mt) => mt,
        Err(e) => {
            state.status_message = format!("MutableText unavailable: {e}");
            return EnterAction::Nothing;
        }
    };
    let cursor = mt.current().len();
    *state.edit_state.lock().unwrap() = Some(EditState {
        block_id: target.block_id,
        field: target.field,
        mt,
        cursor,
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

    let current = edit_state.mt.current();

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
                if let Err(e) = edit_state.mt.apply_local(TextOp::Delete {
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
                if let Err(e) = edit_state.mt.apply_local(TextOp::Delete {
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
                if let Err(e) = edit_state.mt.apply_local(TextOp::Insert {
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
