use std::sync::{Arc, Mutex};

use futures_signals::signal::{ReadOnlyMutable, SignalExt};
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{
    Backspace, Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp,
    OutdentInline, Paste,
};
use gpui_component::menu::PopupMenuItem;
use holon_api::widget_spec::DataRow;
use holon_frontend::cell::{compute_text_delta, CursorBias, DeltaOp, TextDelta};
use holon_frontend::editor_view_model::{
    structural_block_action, EditorAction, EditorKey, EditorViewModel,
};
use holon_frontend::input::{InputAction, WidgetInput};
use holon_frontend::navigation::{Boundary, CursorHint, NavDirection};
use holon_frontend::popup_menu::PopupState;
use holon_frontend::reactive::BuilderServices;

use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::share_ui::ShareTrigger;

use gpui_component::RopeExt;

/// A persistent GPUI view for an editable text field.
///
/// Thin wrapper around `EditorViewModel` (framework-agnostic logic).
/// GPUI-specific responsibilities: InputState entity, GPUI action capture,
/// popup overlay rendering, signal watching, cursor manipulation.
pub struct EditorView {
    input: Entity<InputState>,
    controller: Arc<Mutex<EditorViewModel>>,
    row_id: String,
    services: Arc<dyn BuilderServices>,
    nav: NavigationState,
    /// Cancelled on drop (GPUI `Task` semantics). Owns the data →
    /// InputState propagation task that keeps the editor in sync with
    /// external row updates (peer edits, file reloads, split_block
    /// truncations) without polling on every render. The render path no
    /// longer touches `set_value`.
    _data_subscription: Option<Task<()>>,
    /// Cancelled on drop. Subscribes to the in-memory `focused_block` signal
    /// and grabs window focus when focus becomes this editor's `row_id`
    /// (ADR 0010: window focus follows the signal, never a Turso matview).
    _focus_subscription: Option<Task<()>>,
    /// Snapshot of the text after the last local or remote change.
    /// Used to compute the delta on `InputEvent::Change`. The
    /// `MutableText` itself lives on the `EditorViewModel`.
    previous_text: String,
    /// Cancelled on drop. Subscribes to `MutableText.remote_deltas()`
    /// and splices remote edits into InputState via `replace_text_in_range_silent`.
    _remote_delta_subscription: Option<Task<()>>,
    /// Bounds registry threaded from `GpuiRenderContext` so the popup
    /// overlay can register each item as a tracked widget. Lets the PBT
    /// driver observe the popup state via `wait_for_widget_kind` instead
    /// of poking the EditorViewModel directly.
    bounds_registry: BoundsRegistry,
    /// Last window-focus state observed by the render-path reconcile gate
    /// (`focus_arrived`). Used to detect the frame where focus first arrives
    /// (false→true) so the builder can re-sync a stale `InputState` from the
    /// live backend content *once*, before the user has typed — the backstop
    /// for the editor's data-sync subscription being orphaned by a row-set
    /// rebuild (split/join/navigation replaces the per-row `Mutable` cell).
    prev_focused: std::cell::Cell<bool>,
}

impl EditorView {
    pub fn new(
        _: String,
        content: String,
        field: String,
        row_id: String,
        operations: Vec<holon_api::render_types::OperationWiring>,
        triggers: Vec<holon_frontend::input_trigger::InputTrigger>,
        services: Arc<dyn BuilderServices>,
        nav: NavigationState,
        // Shared per-row data cell from `ReactiveRowSet`. When `Some`, the
        // editor subscribes to it and keeps `InputState` in sync with
        // backend updates. When `None` (snapshot/test paths), the editor
        // shows the initial `content` and never updates from data.
        data: Option<ReadOnlyMutable<Arc<DataRow>>>,
        bounds_registry: BoundsRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let row_id_for_menu = row_id.clone();
            InputState::new(window, cx)
                .auto_grow(1, usize::MAX)
                .default_value(&content)
                .context_menu_extender(move |menu, _window, _cx| {
                    let row_id_for_click = row_id_for_menu.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Share subtree…").on_click(
                            move |_, _window, cx| {
                                ShareTrigger::trigger(row_id_for_click.clone(), cx);
                            },
                        ))
                })
        });

        let context_params = std::collections::HashMap::from([(
            "id".into(),
            holon_api::Value::String(row_id.clone()),
        )]);
        let field_for_subscription = field.clone();
        let mut controller =
            EditorViewModel::new(operations, triggers, context_params, field, content);
        controller.set_async_context(services.clone());
        // Attach a `Cell<String>` if the cell registry can resolve one.
        // Headless / stub / test paths leave it unattached and the VM's
        // pass-through CRDT methods become no-ops.
        // ALLOW(entity_uri_from_raw): boundary — `row_id` is the render-spec row id (a `String`); parse once here before handing a typed URI to the cell registry.
        let row_uri = holon_api::EntityUri::from_raw(&row_id);
        if let Ok(cell) = services.editable_text(&row_uri, &field_for_subscription) {
            controller.attach_cell(cell);
        }
        let controller = Arc::new(Mutex::new(controller));

        // Subscribe to blur and change events.
        {
            let ctrl = controller.clone();
            let services_clone = services.clone();
            let row_id_for_blur = row_id.clone();
            cx.subscribe_in(
                &input,
                window,
                move |this, entity, event, _window, cx| match event {
                    InputEvent::Focus => {
                        #[cfg(feature = "mobile")]
                        gpui_mobile::show_keyboard();

                        // Promote this block to be the UiState.focused_block.
                        // Without this, clicking inside an editable_text gives the
                        // underlying Input gpui-focus but `focused_block` stays on
                        // whatever was focused before — chord keys and operations
                        // then dispatch against the wrong block. PBT inv-focus-matches-ref and the
                        // GeometryDriver read the focus from the engine's
                        // `focused_block_mutable()` Mutable, so this single write
                        // is the only update needed.
                        // ALLOW(entity_uri_from_raw): EditorView.row_id from render-spec node.row_id() (parsed on Focus/Blur)
                        let my_uri = holon_api::EntityUri::from_raw(&row_id_for_blur);
                        if services_clone.focused_block().as_ref() != Some(&my_uri) {
                            if caret_probe() {
                                eprintln!(
                                    "[focus-promote] gpui Focus event on row={my_uri} STEALS \
                                     focused_block from {:?}",
                                    services_clone.focused_block()
                                );
                            }
                            services_clone.set_focus(Some(my_uri));
                        }
                        let _ = (this, entity, cx);
                    }
                    InputEvent::Blur => {
                        #[cfg(feature = "mobile")]
                        gpui_mobile::hide_keyboard();

                        let value = entity.read(cx).value().to_string();
                        let action = ctrl.lock().unwrap().on_blur(&value);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);
                        // Cursor position is no longer persisted on blur: editor
                        // focus + caret are pure in-memory UI state (ADR 0010),
                        // not round-tripped through the Turso `editor_cursor`
                        // matview. The old persist-on-blur existed only to feed
                        // that matview's CDC back into window focus, which is
                        // exactly the steal-back path this removes.
                    }
                    InputEvent::Change => {
                        let text = entity.read(cx).value().to_string();
                        let cursor_pos = entity.read(cx).cursor_position();
                        let cursor_line = cursor_pos.line as usize;
                        let current_line = text.lines().nth(cursor_line).unwrap_or("");
                        // `cursor_position().character` is a CHARACTER column;
                        // `on_text_changed` (→ `check_triggers`) slices the
                        // line by BYTE offset — convert here or multibyte
                        // content panics on a non-char-boundary slice.
                        let cursor_column = cursor_pos.character as usize;
                        let cursor_byte = current_line
                            .char_indices()
                            .nth(cursor_column)
                            .map(|(b, _)| b)
                            .unwrap_or(current_line.len());

                        let action = ctrl
                            .lock()
                            .unwrap()
                            .on_text_changed(current_line, cursor_byte);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);

                        // CRDT: compute local delta and apply through the
                        // view model. The Loro-backed cell filters our own
                        // writes via origin == "ui_local".
                        let vm = ctrl.lock().unwrap();
                        if vm.has_cell() {
                            let prev = this.previous_text.clone();
                            if text != prev {
                                // Only write to the cell when it is actually
                                // behind `text`. A genuine keystroke leaves the
                                // cell lagging `InputState` (the cell is written
                                // only here), so the delta lands. But a
                                // backend-originated change — e.g. a split
                                // truncation projected back through the data
                                // subscription's `set_value` — already wrote the
                                // new content to the cell; re-deriving
                                // `compute_text_delta(prev, text)` against the
                                // stale `prev` and applying it would double-apply
                                // (for a truncation, delete past the new end,
                                // which Loro rejects out-of-bounds). Re-baseline
                                // `previous_text` either way so the next genuine
                                // keystroke diffs from the correct anchor.
                                if vm.current_text().as_deref() != Some(text.as_str()) {
                                    for op in compute_text_delta(&prev, &text) {
                                        if let Err(e) = vm.apply_local(op) {
                                            tracing::error!("apply_local failed: {}", e);
                                        }
                                    }
                                }
                                this.previous_text = text;
                            }
                        }
                        drop(vm);

                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
        }

        // Data → InputState propagation. Subscribes to the shared per-row
        // signal cell from `ReactiveRowSet` and applies external row
        // changes (peer edits, file reloads, split_block truncations,
        // CDC echoes of our own writes) into the local InputState.
        //
        // Two safeguards:
        //
        // 1. **Skip when focused.** While the user has the editor focused
        //    they are the source of truth — overwriting `InputState`
        //    while they're typing yanks the cursor to position 0 and
        //    drops the in-flight character. External changes during a
        //    focused edit are dropped from the *visible* state until the
        //    next focus cycle (data is still correct in the backend).
        //
        // 2. **Dedupe on the field's value.** The signal fires on every
        //    `.set()` of the per-row Mutable, including no-op writes
        //    triggered by unrelated field changes. `.dedupe_cloned()` on
        //    the extracted field value keeps the subscription quiet
        //    unless the relevant column actually changed.
        //
        // The render path no longer touches `set_value` — propagation is
        // entirely event-driven through this subscription. The returned
        // `Task<()>` cancels on drop, so removing this `EditorView`
        // (e.g. via collection driver `RemoveAt`) tears the subscription
        // down naturally.
        let _data_subscription: Option<Task<()>> = data.map(|data_handle| {
            let field_for_stream = field_for_subscription.clone();
            let signal = data_handle
                .signal_cloned()
                .map(move |row| {
                    row.get(&field_for_stream)
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string()
                })
                .dedupe_cloned();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = signal.to_stream();
                // No unconditional initial drop: when this EditorView is
                // reused from cache for a row whose content changed, the
                // first emission is the *new* value, and dropping it would
                // strand the widget on stale text. The loop body's
                // value-equality guard already makes redundant emissions a
                // no-op, so let the same gate apply to the first one.
                //
                // `last_synced` tracks the last value we *ourselves* set
                // (or observed-and-skipped because state already matched).
                // Comparing current InputState against `last_synced` lets
                // us tell apart "user has typed since the last sync" (state
                // diverged) from "user is focused but idle" (state equals
                // last sync). External updates apply in the idle case
                // regardless of focus — that's how post-`split_block`
                // truncations and other structural mutations land while the
                // editor still owns focus, without ever yanking text the
                // user is mid-typing.
                let mut last_synced: Option<String> = None;
                while let Some(new_value) = stream.next().await {
                    if this.upgrade().is_none() {
                        // EditorView dropped (e.g. row removed by
                        // collection driver). Stop the loop — the `Task`
                        // will be dropped shortly when our owning struct
                        // is freed, but exiting cleanly avoids a tight
                        // spin while the Drop runs.
                        break;
                    }
                    let prev_synced = last_synced.clone();
                    let mut applied = false;
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        let input = view.read(cx).input.clone();
                        // Focus is window-scoped; pick the first window
                        // that owns this input entity. There is exactly
                        // one in normal app usage.
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                input.update(cx, |state, cx| {
                                    let current = state.value().to_string();
                                    if current == new_value {
                                        // Already in sync (CDC echo of our
                                        // own write or a redundant emit).
                                        applied = true;
                                        return;
                                    }
                                    let focused = state.focus_handle(cx).is_focused(window);
                                    // "User is idle": InputState matches the
                                    // last-seen committed value, so any
                                    // typing they had pending was already
                                    // flushed. External updates safe.
                                    let user_idle =
                                        prev_synced.as_deref() == Some(current.as_str());
                                    if focused && !user_idle {
                                        // Mid-typing — skip to avoid cursor
                                        // yank. `last_synced` deliberately
                                        // not advanced; we'll catch up on
                                        // the next emission once the user
                                        // commits or the values reconverge.
                                        if caret_probe() {
                                            eprintln!(
                                                "[data-sync] SKIP (focused, not idle) \
                                                 current={current:?} new={new_value:?} \
                                                 prev_synced={prev_synced:?}"
                                            );
                                        }
                                        return;
                                    }
                                    if caret_probe() {
                                        eprintln!(
                                            "[data-sync] apply current={current:?} \
                                             new={new_value:?}"
                                        );
                                    }
                                    state.set_value(&new_value, window, cx);
                                    applied = true;
                                });
                            });
                        }
                    });
                    if applied {
                        last_synced = Some(new_value);
                    }
                }
            })
        });

        // Window focus follows the in-memory `focused_block` authority
        // (ADR 0010): grab window focus whenever focus becomes this row.
        // Editor focus is never read back from Turso, so a late SQL
        // re-emission can't steal focus. Handles focus arriving at an
        // already-mounted (cache-reused) editor; the synchronous first-mount
        // grab below covers the fast path. RAII-scoped to this EditorView.
        // ALLOW(entity_uri_from_raw): render-spec row_id parsed once to match the focus signal
        let row_uri_for_focus = holon_api::EntityUri::from_raw(&row_id);
        let _focus_subscription = spawn_focus_binding(cx, services.clone(), row_uri_for_focus);

        // ── CRDT-backed remote delta subscription ──────
        //
        // When the view model has an attached `Cell<String>`, seed the
        // diff baseline from its current text and subscribe to remote
        // deltas. Cursor preservation uses Loro's `Cursor` anchoring via
        // the VM's `anchor_cursor` / `resolve_cursor` pass-throughs.
        let _ = field_for_subscription;
        let previous_text = controller
            .lock()
            .unwrap()
            .current_text()
            .unwrap_or_default();
        let cell_for_remote = controller.lock().unwrap().cell().cloned();
        let input_for_remote = input.clone();
        let _remote_delta_subscription: Option<Task<()>> = cell_for_remote.map(|cell| {
            let _input = input_for_remote.clone();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = cell.remote_deltas();
                while let Some(delta) = stream.next().await {
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        let editor_input = view.read(cx).input.clone();
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                let state = editor_input.read(cx);
                                // IME guard: skip while composition is active
                                if state.ime_marked_range().is_some() {
                                    return;
                                }
                                // Anchor cursor before applying remote delta.
                                let cursor_codepoint =
                                    state.text().offset_to_char_index(state.cursor());
                                let anchor =
                                    cell.anchor_cursor(cursor_codepoint, CursorBias::Left).ok(); // ALLOW(ok): backings without text-rich support degrade to None; cursor restoration falls back to 0
                                                                                                 // Release the immutable borrow before updating
                                let _state = state;
                                editor_input.update(cx, |state, cx| {
                                    apply_text_delta_to_state(state, &delta, window, cx);
                                });
                                // Resolve cursor after applying remote delta.
                                let new_codepoint = anchor
                                    .as_ref()
                                    .and_then(|a| cell.resolve_cursor(a).ok()) // ALLOW(ok) ALLOW(fallback): backings without text-rich support degrade to position 0
                                    .unwrap_or(0);
                                editor_input.update(cx, |state, cx| {
                                    let byte_offset =
                                        state.text().char_index_to_offset(new_codepoint);
                                    let pos = state.text().offset_to_position(byte_offset);
                                    state.set_cursor_position(pos, window, cx);
                                });
                            });
                        }
                    });
                }
            })
        });

        // First-mount focus grab. The block_profile.yaml variant switch
        // ("editing" vs "default") mounts this editor only when
        // `is_focused == true`, so by the time we're constructed the
        // intended-focused block is already this one. The async focus
        // subscription is too slow for the PBT click-then-keystroke pipeline
        // (`SplitBlock` sends `home` immediately after focus matches), so
        // grab synchronously here when `focused_block` already matches. The
        // caret seed (split → 0, join → boundary, nav → placement) is applied
        // by the shared helper; with no armed seed, a genuinely fresh editor
        // has no meaningful caret and defaults to end-of-text. Keystrokes the
        // driver/user pressed before this mount cannot have landed in this
        // editor: blur-on-focus-leave releases the stale editor's window
        // focus, so pre-mount keys are dropped (and retried by the driver),
        // never consumed at the wrong caret — the end default cannot yank a
        // caret the user already placed, because no user interaction can have
        // reached a not-yet-mounted InputState.
        // ALLOW(entity_uri_from_raw): render-spec row_id parsed vs focused_block() on mount
        let row_uri = holon_api::EntityUri::from_raw(&row_id);
        if services.focused_block().as_ref() == Some(&row_uri) {
            grab_focus_and_seed_caret(&input, window, cx, services.as_ref(), &row_uri, true);
        }

        Self {
            input,
            controller,
            row_id,
            services,
            nav,
            bounds_registry,
            _data_subscription,
            _focus_subscription,
            previous_text,
            _remote_delta_subscription,
            prev_focused: std::cell::Cell::new(false),
        }
    }

    /// Update the render-path focus-transition tracker and report whether
    /// window focus *just arrived* this frame (false→true). The builder uses
    /// this to allow a one-time external-content reconcile into a
    /// freshly-focused editor (e.g. click-to-edit) before any keystroke,
    /// without clobbering text a continuously-focused user is mid-typing.
    pub fn focus_arrived(&self, is_focused: bool) -> bool {
        let just = is_focused && !self.prev_focused.get();
        self.prev_focused.set(is_focused);
        just
    }
}

/// Bind this editor's window focus to the in-memory `focused_block` signal:
/// whenever focus becomes `row`, grab window focus and seed the caret. This is
/// the single place the signal→`window.focus` bridge lives — gpui has no
/// declarative focus binding, so the spawn is irreducible, but it is contained
/// here. Returns `None` when there is no focus authority (headless/stub
/// services). The returned `Task` is RAII-scoped to the storing `EditorView`
/// and cancels on drop, so there is no manual unsubscribe.
fn spawn_focus_binding(
    cx: &mut Context<EditorView>,
    services: Arc<dyn BuilderServices>,
    row: holon_api::EntityUri,
) -> Option<Task<()>> {
    use futures_signals::signal::SignalExt;
    let focus_mutable = services.focused_block_mutable()?;
    // Reduce the focus signal to a deduped "am I focused?" boolean so the
    // effect only fires when focus actually arrives at (or leaves) this row —
    // never on unrelated focus churn, and never mid-typing.
    let row_for_signal = row.clone();
    let is_focused = focus_mutable
        .signal_cloned()
        .map(move |f| f.as_ref() == Some(&row_for_signal))
        .dedupe();
    Some(cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        let mut stream = is_focused.to_stream();
        while let Some(focused) = stream.next().await {
            if this.upgrade().is_none() {
                break;
            }
            let _ = cx.update(|cx| {
                let Some(view) = this.upgrade() else {
                    return;
                };
                let input = view.read(cx).input.clone();
                if !focused {
                    // The focus authority left this editor: commit its
                    // user-authored pending text NOW. gpui's on_blur event
                    // only fires reliably when the window is key/active — in
                    // a non-key window pending SqlOnly text otherwise stays
                    // uncommitted indefinitely (activation-dependent data
                    // loss, 2026-06-11). The authority move is the
                    // deterministic commit boundary; the gpui blur event is
                    // just a hint (docs/Architecture/UI.md). Idempotent with
                    // the on_blur path: whichever runs first re-baselines,
                    // the second sees no pending change.
                    let live_text = input.read(cx).value().to_string();
                    let ctrl = view.read(cx).controller.clone();
                    let commit = ctrl.lock().unwrap().pending_commit_intent(&live_text);
                    if let Some(commit) = commit {
                        services.dispatch_intent(commit);
                    }
                }
                for window_handle in cx.windows() {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        if focused {
                            // Fires on every focus arrival, including after the
                            // user/driver already positioned the caret — applies
                            // only an armed seed, never resets an unseeded caret.
                            grab_focus_and_seed_caret(
                                &input,
                                window,
                                cx,
                                services.as_ref(),
                                &row,
                                false,
                            );
                        } else if blur_on_focus_leave()
                            && input.read(cx).focus_handle(cx).is_focused(window)
                        {
                            // The authority moved away (split/join op response,
                            // navigation) but this editor still holds WINDOW
                            // focus — the new block's editor may not have
                            // mounted yet, so its grab can't have happened. A
                            // keystroke landing in this gap would be consumed
                            // by the stale editor and mutate the WRONG block
                            // (the zombie-editor race). Releasing focus now
                            // means such a keystroke is dropped, not
                            // misdelivered; the new editor's first-mount grab /
                            // focus binding picks focus up.
                            //
                            window.blur();
                        }
                    });
                }
            });
        }
    }))
}

/// Blur the window when the focus authority leaves a still-window-focused
/// editor (the zombie-editor fix; see the call site in
/// [`spawn_focus_binding`]). Default ON; `HOLON_GPUI_BLUR_ON_FOCUS_LEAVE=0`
/// is the kill-switch (used to A/B the behavior under PBT — the 2026-06-10
/// causality test showed the PBT gate failures occur identically with it
/// off, exonerating this fix).
fn blur_on_focus_leave() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("HOLON_GPUI_BLUR_ON_FOCUS_LEAVE").as_deref() != Ok("0"))
}

/// `HOLON_GPUI_CARET_PROBE=1` logs every caret-seed decision (armed seed vs
/// end-default vs leave-alone) with the editor text and window activation
/// state — the discriminator for split-at-wrong-position divergences.
fn caret_probe() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("HOLON_GPUI_CARET_PROBE").as_deref() == Ok("1"))
}

/// "Structural ops are commit points" (docs/Architecture/UI.md): flush any
/// pending editor text, then run the structural op, as ONE ordered dispatch
/// chain. The op must compute against the authority's current content — a
/// split against backend content with the editor's cursor byte fails on (or
/// silently mis-splits) text that is still pending in the editor ("Split
/// position 8 exceeds content length 3", 2026-06-11). When Loro's
/// per-keystroke writer is active or the text is unchanged the commit is
/// `None` and this degenerates to a plain dispatch.
fn dispatch_structural_as_commit_point(
    ctrl: &Arc<Mutex<EditorViewModel>>,
    services: &Arc<dyn BuilderServices>,
    live_text: &str,
    structural: holon_frontend::operations::OperationIntent,
) {
    let commit = ctrl.lock().unwrap().pending_commit_intent(live_text);
    let intents: Vec<_> = commit
        .into_iter()
        .chain(std::iter::once(structural))
        .collect();
    holon_frontend::reactive::dispatch_intent_chain(services, intents);
}

/// Grab window keyboard focus for `input` (if it doesn't already own it) and
/// place the caret at the pending seed offset armed for `row`.
///
/// An explicitly armed seed (split → 0, join → boundary, nav → placement)
/// always wins. With no seed, the caret's owner depends on the caller:
///
/// - `default_caret_to_end = true` — the synchronous first-mount grab. A
///   genuinely fresh editor has no meaningful caret, so default end-of-text
///   (matches the PBT ref's `model_chord_click_focus` and the headless
///   mirror's `seed_for_click`). Pre-mount keystrokes can't have placed a
///   caret here: blur-on-focus-leave drops them and the driver retries, so
///   nothing user-placed exists to be yanked.
/// - `default_caret_to_end = false` — the async focus subscription, which
///   re-fires on every focus arrival, *after* `home`+arrow keys may already
///   have moved the caret. An end-default there yanked the caret back to
///   the end, so `Enter` split at the end (source kept its full content,
///   new block empty — the SplitBlock-at-wrong-position bug). Leave an
///   unseeded caret alone.
///
/// `peek_caret_seed` is non-destructive, so applying an armed seed from
/// both callers is idempotent.
fn grab_focus_and_seed_caret(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
    services: &dyn BuilderServices,
    row: &holon_api::EntityUri,
    default_caret_to_end: bool,
) {
    if !input.read(cx).focus_handle(cx).is_focused(window) {
        window.focus(&input.read(cx).focus_handle(cx), cx);
    }
    let seed = services.peek_caret_seed(row);
    input.update(cx, |state, cx| {
        if caret_probe() {
            eprintln!(
                "[caret-seed] row={row} seed={seed:?} default_end={default_caret_to_end} \
                 text={:?} window_active={}",
                state.text().to_string(),
                window.is_window_active(),
            );
        }
        if let Some(offset) = seed {
            let pos = state.text().offset_to_position(offset);
            state.set_cursor_position(pos, window, cx);
        } else if default_caret_to_end {
            let end = state.text().len();
            let pos = state.text().offset_to_position(end);
            state.set_cursor_position(pos, window, cx);
        }
    });
}

impl EditorView {
    pub fn row_id(&self) -> &str {
        &self.row_id
    }

    pub fn input_entity(&self) -> &Entity<InputState> {
        &self.input
    }
}

impl Render for EditorView {
    #[tracing::instrument(
        level = "trace",
        skip_all,
        name = "frontend.render",
        fields(component = "editor")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor_entity_id = self.input.entity_id();
        let popup_overlay = {
            let ctrl = self.controller.lock().unwrap();
            ctrl.popup_state()
                .map(|s| render_popup(&s, &self.bounds_registry, cx))
        };

        let window_handle = window.window_handle();

        div()
            .w_full()
            .relative()
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &MoveUp, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Up);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &MoveDown, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Down);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                let input = self.input.clone();
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &Enter, window, cx: &mut App| {
                    // Two layered guards keep Enter on a Page-level editor
                    // from acting on behalf of a focused child:
                    //   1. only the editor whose own InputState owns
                    //      keyboard focus runs the capture body, and
                    //   2. the operation targets `services.focused_block()`
                    //      (UiState's notion of focus), not this editor's
                    //      own `row_id` — so even if the capture fires on a
                    //      shared/ancestor editor, the split lands on the
                    //      logically focused leaf.
                    if !input.read(cx).focus_handle(cx).is_focused(window) {
                        return;
                    }
                    let target_id = services
                        .focused_block()
                        .map(|u| u.as_str().to_string())
                        .unwrap_or_else(|| row_id.clone());
                    // Cmd+Enter → dispatch cycle_task_state.
                    // GPUI's action system captures Enter before on_key_down fires,
                    // so we handle the keychord here directly.
                    if window.modifiers().platform {
                        let mut params = std::collections::HashMap::new();
                        params.insert("id".into(), holon_api::Value::String(target_id.clone()));
                        services.dispatch_intent(holon_frontend::operations::OperationIntent::new(
                            "block".into(),
                            "cycle_task_state".into(),
                            params,
                        ));
                        cx.stop_propagation();
                        return;
                    }
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Enter);
                    match action {
                        EditorAction::InsertText {
                            replacement,
                            prefix_start,
                        } => {
                            let text = input.read(cx).value().to_string();
                            let cursor = input.read(cx).cursor();
                            let cursor_pos = input.read(cx).cursor_position();
                            let line_start = cursor - cursor_pos.character as usize;
                            let abs_start = line_start + prefix_start;

                            let mut new_text =
                                String::with_capacity(text.len() + replacement.len());
                            new_text.push_str(&text[..abs_start]);
                            new_text.push_str(&replacement);
                            new_text.push_str(&text[cursor..]);
                            let new_cursor_offset = abs_start + replacement.len();

                            let input = input.clone();
                            cx.spawn(async move |cx| {
                                let _ = cx.update_window(window_handle, |_, window, cx| {
                                    input.update(cx, |state, cx| {
                                        state.set_value(&new_text, window, cx);
                                        let pos =
                                            state.text().offset_to_position(new_cursor_offset);
                                        state.set_cursor_position(pos, window, cx);
                                    });
                                });
                            })
                            .detach();
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::Execute(intent) => {
                            services.dispatch_intent(intent);
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::ExecuteAndStripCommand {
                            intent,
                            strip_prefix_start,
                        } => {
                            // Remove the typed slash-command text ("/delete")
                            // before dispatching — same span arithmetic as the
                            // InsertText arm, with an empty replacement.
                            // Without this the command text stays in the
                            // editor and is committed to the block at the
                            // next commit point (Loro-twin PBT face,
                            // 2026-06-11: ref "😀" vs SUT "😀/delete").
                            let text = input.read(cx).value().to_string();
                            let cursor = input.read(cx).cursor();
                            // BYTE offset of line start (cursor_position().
                            // character is a CHAR column — subtracting it from
                            // the byte cursor breaks on multibyte content).
                            let line_start = text[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
                            let abs_start = line_start + strip_prefix_start;
                            if caret_probe() {
                                eprintln!(
                                    "[slash-strip] windowed arm: text={text:?} cursor={cursor} \
                                     abs_start={abs_start}"
                                );
                            }
                            let mut new_text = String::with_capacity(text.len());
                            new_text.push_str(&text[..abs_start]);
                            new_text.push_str(&text[cursor..]);

                            let input = input.clone();
                            cx.spawn(async move |cx| {
                                let _ = cx.update_window(window_handle, |_, window, cx| {
                                    input.update(cx, |state, cx| {
                                        state.set_value(&new_text, window, cx);
                                        let pos = state.text().offset_to_position(abs_start);
                                        state.set_cursor_position(pos, window, cx);
                                    });
                                });
                            })
                            .detach();
                            services.dispatch_intent(intent);
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::PopupDismissed | EditorAction::UpdatePopup => {
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::None => {
                            // No popup active → split the block at the cursor.
                            // We can't rely on Enter bubbling to lib.rs's chord
                            // resolver: gpui-component's InputState consumes
                            // Enter for multi-line newline insertion (auto_grow
                            // sets max_rows > 1, making is_multi_line() true),
                            // so the bubble-phase on_action never fires. The
                            // Enter→split decision is shared with the headless
                            // test mirror via `structural_block_action`.
                            let cursor_byte = input.read(cx).cursor();
                            if caret_probe() {
                                eprintln!(
                                    "[split-dispatch] target={target_id} cursor={cursor_byte} \
                                     editor_text={:?}",
                                    input.read(cx).value().to_string()
                                );
                            }
                            if let Some(intent) =
                                structural_block_action(EditorKey::Enter, &target_id, cursor_byte)
                            {
                                let live_text = input.read(cx).value().to_string();
                                dispatch_structural_as_commit_point(
                                    &ctrl, &services, &live_text, intent,
                                );
                            }
                            cx.stop_propagation();
                        }
                        EditorAction::Propagate => {
                            cx.propagate();
                        }
                        EditorAction::PopupActivated { .. } => {
                            // Enter shouldn't activate a popup, but handle gracefully
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &Escape, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Escape);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            // Intercept Backspace at cursor position 0 → join_block.
            // Anywhere else (cursor > 0), let `InputState` consume it for
            // its normal char-delete. The chord pipeline can't decide
            // this — only the live editor knows the cursor offset — so
            // GPUI dispatches the intent directly here, mirroring the
            // Enter → split_block pattern below.
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                let input = self.input.clone();
                let ctrl = self.controller.clone();
                move |_: &Backspace, _window, cx: &mut App| {
                    let cursor_byte = input.read(cx).cursor();
                    if cursor_byte != 0 {
                        // Not at start — let InputState handle char delete.
                        return;
                    }
                    // Backspace-at-0 → join. Decision shared with the headless
                    // mirror via `structural_block_action`.
                    if let Some(intent) = structural_block_action(EditorKey::Backspace, &row_id, 0)
                    {
                        let live_text = input.read(cx).value().to_string();
                        dispatch_structural_as_commit_point(&ctrl, &services, &live_text, intent);
                    }
                    cx.stop_propagation();
                }
            })
            // Intercept Tab/Shift+Tab before InputState consumes them for
            // tab-character insertion. Dispatch indent/outdent directly,
            // matching the Enter → split_block pattern above.
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                let input = self.input.clone();
                let ctrl = self.controller.clone();
                move |_: &IndentInline, _window, cx: &mut App| {
                    if let Some(intent) = structural_block_action(EditorKey::Tab, &row_id, 0) {
                        let live_text = input.read(cx).value().to_string();
                        dispatch_structural_as_commit_point(&ctrl, &services, &live_text, intent);
                    }
                    cx.stop_propagation();
                }
            })
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                let input = self.input.clone();
                let ctrl = self.controller.clone();
                move |_: &OutdentInline, _window, cx: &mut App| {
                    if let Some(intent) = structural_block_action(EditorKey::BackTab, &row_id, 0) {
                        let live_text = input.read(cx).value().to_string();
                        dispatch_structural_as_commit_point(&ctrl, &services, &live_text, intent);
                    }
                    cx.stop_propagation();
                }
            })
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &Paste, _window, cx: &mut App| {
                    if let Some(clipboard) = cx.read_from_clipboard() {
                        for entry in clipboard.entries() {
                            if let ClipboardEntry::Image(image) = entry {
                                let ext = match image.format {
                                    ImageFormat::Png => "png",
                                    ImageFormat::Jpeg => "jpeg",
                                    ImageFormat::Gif => "gif",
                                    ImageFormat::Webp => "webp",
                                    ImageFormat::Svg => "svg",
                                    ImageFormat::Bmp => "bmp",
                                    ImageFormat::Tiff => "tiff",
                                    ImageFormat::Ico => "ico",
                                };
                                match save_clipboard_image(&image.bytes, ext) {
                                    Ok(relative_path) => {
                                        let new_id = holon_api::EntityUri::block_random();
                                        let mut params = std::collections::HashMap::new();
                                        params.insert(
                                            "id".into(),
                                            holon_api::Value::String(new_id.to_string()),
                                        );
                                        params.insert(
                                            "content".into(),
                                            holon_api::Value::String(relative_path),
                                        );
                                        params.insert(
                                            "content_type".into(),
                                            holon_api::Value::String("image".into()),
                                        );
                                        params.insert(
                                            "after".into(),
                                            holon_api::Value::String(row_id.clone()),
                                        );
                                        services.dispatch_intent(
                                            holon_frontend::operations::OperationIntent::new(
                                                "block".into(),
                                                "create".into(),
                                                params,
                                            ),
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to save pasted image: {e}");
                                    }
                                }
                                cx.stop_propagation();
                                return;
                            }
                        }
                    }
                }
            })
            // Cross-block navigation. InputState consumes MoveUp/MoveDown for
            // cursor movement; at the top/bottom boundary it `cx.propagate()`s
            // them. The bubble-phase handlers below catch that boundary
            // bubble and ask the input router for the next focusable block.
            .on_action({
                let nav = self.nav.clone();
                let services = self.services.clone();
                let input = self.input.clone();
                let row_id = self.row_id.clone();
                move |_: &MoveUp, _window, cx: &mut App| {
                    handle_cross_block_nav(
                        &nav,
                        &services,
                        &row_id,
                        &input,
                        NavDirection::Up,
                        Boundary::Top,
                        cx,
                    );
                }
            })
            .on_action({
                let nav = self.nav.clone();
                let services = self.services.clone();
                let input = self.input.clone();
                let row_id = self.row_id.clone();
                move |_: &MoveDown, _window, cx: &mut App| {
                    handle_cross_block_nav(
                        &nav,
                        &services,
                        &row_id,
                        &input,
                        NavDirection::Down,
                        Boundary::Bottom,
                        cx,
                    );
                }
            })
            .child(Input::new(&self.input).appearance(false))
            .when_some(popup_overlay, |d, overlay| d.child(overlay))
    }
}

/// Handle a MoveUp/MoveDown that bubbled up from this editor's `InputState`
/// at its top/bottom boundary. Asks the input router for the next focusable
/// block, then dispatches a `navigation::editor_focus` operation. The target
/// editor's own cursor-signal subscription receives the resulting CDC fire
/// and applies focus + cursor offset against its own `InputState`.
///
/// Reads the target's current text from the engine snapshot (not from the
/// target's `InputState`) so we don't need a global registry of editors.
#[tracing::instrument(level = "debug", skip_all, fields(?direction, source = %row_id))]
fn handle_cross_block_nav(
    nav: &NavigationState,
    services: &Arc<dyn BuilderServices>,
    row_id: &str,
    input: &Entity<InputState>,
    direction: NavDirection,
    boundary: Boundary,
    cx: &mut App,
) {
    let column = input.read(cx).cursor_position().character as usize;
    let hint = CursorHint { column, boundary };
    let widget_input = WidgetInput::Navigate { direction, hint };

    // Editor row ids are schemed (set from the rendered row's `id`) — a
    // non-URI here is a programming error, fail loud.
    let row_uri =
        holon_api::EntityUri::parse(row_id).expect("editor row_id must be a schemed EntityUri");
    match nav.bubble_input(&row_uri, &widget_input) {
        Some(InputAction::Focus {
            block_id,
            placement,
        }) => {
            // Resolve the target's current text from the engine so we can
            // turn `placement` into a byte offset without poking at the
            // target's `InputState`. Content in the matview is the same
            // text the target editor renders (it propagates via the
            // per-editor data subscription on every `Change`).
            let target_uri = block_id;
            let (_render, rows) = services.get_block_data(&target_uri);
            let target_text = rows
                .first()
                .and_then(|r| r.get("content"))
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_string();
            let offset = holon_frontend::navigation::placement_to_offset(&target_text, placement);

            // Move focus to the target at the placement offset, in memory
            // (ADR 0010). `set_focus_with_caret` arms the caret seed the
            // target editor reads on mount — no `editor_cursor` write, no CDC
            // round-trip. The target may be a cache-reused editor, so the
            // async focus subscription (not the first-mount grab) applies it.
            services.set_focus_with_caret(target_uri, offset);
            cx.stop_propagation();
        }
        Some(other) => {
            tracing::debug!("cross_block_nav: bubble_input returned non-Focus action: {other:?}");
        }
        None => {
            tracing::debug!(
                "cross_block_nav: bubble_input returned None for row_id={row_id}, direction={direction:?} (router={})",
                nav.describe()
            );
        }
    }
}

/// Render the unified popup overlay.
///
/// Each visible popup item is wrapped in `crate::geometry::tracked` so it
/// registers in `BoundsRegistry` as `widget_type="popup_item"` with
/// `entity_id = item.id` (and `el_id = "popup-item-{item.id}"`). Lets PBT
/// drivers observe the popup via `wait_for_widget_kind` / element lookups
/// instead of poking the EditorViewModel directly. The currently-
/// highlighted item is also tagged `widget_type="popup_item_selected"`
/// so a precondition can confirm Enter would fire the expected op.
fn render_popup(state: &PopupState, bounds_registry: &BoundsRegistry, cx: &App) -> Deferred {
    use gpui::prelude::*;
    use gpui::{div, px};
    use gpui_component::theme::ActiveTheme;

    let theme = cx.theme().colors;
    let bg = theme.popover;
    let border = theme.border;
    let text_color = theme.foreground;
    let selected_bg = theme.accent;
    let selected_text = theme.accent_foreground;
    let muted = theme.muted_foreground;

    let mut container = div()
        .absolute()
        .left_0()
        .top(px(20.0))
        .w(px(280.0))
        .max_h(px(240.0))
        .overflow_y_hidden()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded(px(6.0))
        .shadow_md()
        .p_1()
        .flex_col()
        .text_color(text_color)
        .text_sm();

    if state.items.is_empty() {
        container = container.child(
            div()
                .px_2()
                .py_1()
                .text_color(muted)
                .child("Type to search..."),
        );
    } else {
        for (i, item) in state.items.iter().enumerate() {
            let is_selected = i == state.selected_index;
            let mut row = div()
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .when(is_selected, |d| d.bg(selected_bg).text_color(selected_text));

            if let Some(icon) = &item.icon {
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(icon.clone())
                        .child(item.label.clone()),
                );
            } else {
                row = row.child(item.label.clone());
            }
            let widget_type = if is_selected {
                "popup_item_selected"
            } else {
                "popup_item"
            };
            // Canonicalize through the same total boundary helper the PBT
            // driver uses: `PopupItem.id` is a raw token (op name for slash
            // commands, bare block id for link candidates), but waits compare
            // against schemed `EntityUri` strings — registering the raw token
            // made `wait_for_widget_kind(EntityUri::block("delete"), ...)`
            // unable to ever match ("delete" vs "block:delete", 4/4
            // deterministic Loro-twin red, 2026-06-11).
            let entity_uri = holon_api::entity_uri_from_id_str(&item.id);
            let tracked = crate::geometry::tracked(
                format!("popup-item-{}", item.id),
                row.into_any_element(),
                bounds_registry,
                widget_type,
                Some(entity_uri.as_str()),
                true,
                Some(std::sync::Arc::from(item.label.as_str())),
            );
            container = container.child(tracked);
        }
    }

    deferred(container).with_priority(1)
}

/// Save clipboard image bytes to the org attachments directory.
/// Returns the relative path (e.g. "attachments/a1b2c3d4.png").
fn save_clipboard_image(bytes: &[u8], extension: &str) -> Result<String, std::io::Error> {
    let root = org_root_dir();
    let attachments = root.join("attachments");
    std::fs::create_dir_all(&attachments)?;

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let hash = hasher.finish();
    let filename = format!("{hash:016x}.{extension}");
    let abs_path = attachments.join(&filename);

    if !abs_path.exists() {
        std::fs::write(&abs_path, bytes)?;
        tracing::info!("Saved pasted image to {}", abs_path.display());
    }
    Ok(format!("attachments/{filename}"))
}

fn org_root_dir() -> std::path::PathBuf {
    if let Ok(root) = std::env::var("HOLON_ORGMODE_ROOT_DIRECTORY") {
        return std::path::PathBuf::from(root);
    }
    if let Ok(root) = std::env::var("HOLON_WORKSPACE_ROOT") {
        return std::path::PathBuf::from(root);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        return std::path::PathBuf::from(manifest);
    }
    std::path::PathBuf::from(".")
}

/// Execute an EditorAction in a context without window access (subscribe callbacks).
fn execute_action<T: 'static>(
    action: EditorAction,
    services: &Arc<dyn BuilderServices>,
    editor_entity_id: EntityId,
    cx: &mut Context<T>,
) {
    match action {
        EditorAction::PopupActivated { signal } => {
            cx.spawn(async move |_this, cx| {
                use futures_signals::signal::SignalExt;
                signal
                    .for_each(|_items| {
                        cx.update(|cx| {
                            cx.notify(editor_entity_id);
                        });
                        async {}
                    })
                    .await;
            })
            .detach();
        }
        EditorAction::Execute(intent) => {
            services.dispatch_intent(intent);
        }
        EditorAction::ExecuteAndStripCommand { intent, .. } => {
            // No-window context: dispatch only; the strip (if any) is handled
            // by the windowed Enter arm, which is where popup Enter arrives.
            // If this probe fires, popup Enter took THIS path and the strip
            // was skipped — that's the bug to chase.
            if caret_probe() {
                eprintln!("[slash-strip] NO-WINDOW arm hit — strip SKIPPED");
            }
            services.dispatch_intent(intent);
        }
        // UpdatePopup, Dismissed, InsertText, None, Propagate — no action needed
        // in the no-window context (subscribe callbacks). The caller handles cx.notify().
        _ => {}
    }
}

/// Apply a `TextDelta` to an `InputState` via `replace_text_in_range_silent`.
///
/// Converts Loro codepoint positions to UTF-16 positions using `RopeExt`.
fn apply_text_delta_to_state(
    state: &mut InputState,
    delta: &TextDelta,
    window: &mut Window,
    cx: &mut Context<InputState>,
) {
    let text_rope = state.text();
    let full_text = text_rope.to_string();

    let mut codepoint_pos = 0usize;
    // Pre-compute char_idx → utf16 offset for the current text
    let char_to_utf16 =
        |cp: usize, s: &str| -> usize { s.chars().take(cp).map(|c| c.len_utf16()).sum() };

    for op in &delta.ops {
        match op {
            DeltaOp::Retain { len_codepoint } => {
                codepoint_pos += len_codepoint;
            }
            DeltaOp::Insert { text } => {
                let utf16 = char_to_utf16(codepoint_pos, &full_text);
                let range = utf16..utf16;
                state.replace_text_in_range_silent(Some(range), text, window, cx);
                codepoint_pos += text.chars().count();
            }
            DeltaOp::Delete { len_codepoint } => {
                let utf16_start = char_to_utf16(codepoint_pos, &full_text);
                let utf16_end = char_to_utf16(codepoint_pos + len_codepoint, &full_text);
                state.replace_text_in_range_silent(Some(utf16_start..utf16_end), "", window, cx);
            }
        }
    }
}
