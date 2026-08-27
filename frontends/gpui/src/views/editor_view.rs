use std::sync::Arc;
use std::sync::Mutex;

use futures_signals::signal::ReadOnlyMutable;
use futures_signals::signal::SignalExt;
use gpui::prelude::*;
use gpui::*;
use gpui_component::RopeExt;
use gpui_component::input::Backspace;
use gpui_component::input::Enter;
use gpui_component::input::Escape;
use gpui_component::input::IndentInline;
use gpui_component::input::Input;
use gpui_component::input::InputEvent;
use gpui_component::input::InputState;
use gpui_component::input::MoveDown;
use gpui_component::input::MoveUp;
use gpui_component::input::OutdentInline;
use gpui_component::input::Paste;
use gpui_component::menu::PopupMenuItem;
use holon_api::widget_spec::DataRow;
use holon_frontend::cell::CursorBias;
use holon_frontend::editor_view_model::ConvergeDirective;
use holon_frontend::editor_view_model::EditorAction;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::editor_view_model::StructuralCaret;
use holon_frontend::editor_view_model::structural_block_action;
use holon_frontend::input::InputAction;
use holon_frontend::input::WidgetInput;
use holon_frontend::navigation::Boundary;
use holon_frontend::navigation::CursorHint;
use holon_frontend::navigation::NavDirection;
use holon_frontend::popup_menu::PopupState;
use holon_frontend::reactive::BuilderServices;

use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::share_ui::ShareTrigger;

/// A persistent GPUI view for an editable text field.
///
/// Thin render/IO adapter, NOT a buffer owner. `EditorView` translates GPUI
/// input events into `EditorViewModel` calls and reflects the VM's decisions
/// back into the `InputState` entity it drives — it holds no buffer text and
/// no sequence counter of its own (the `previous_text` / `last_local_seq`
/// authority fields were removed in Increment 2).
///
/// Authority split:
/// - `EditorViewModel` (framework-agnostic) owns the buffer, the local write
///   sequence, and the echo/convergence policy that decides when an incoming
///   external value is adopted, converged, or ignored as a self-echo.
/// - `EditorView` owns only GPUI/window state: the `InputState` entity, GPUI
///   action capture, IME (`EntityInputHandler`) wiring, window focus, the
///   slash/link popup overlay, and the subscriptions that splice external
///   row/peer/remote-delta updates into `InputState` off the render path.
///
/// The render path never mutates the buffer: it reads VM decisions and applies
/// them to `InputState`, keeping the VM the single source of truth for text.
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
    /// Cancelled on drop. Subscribes to `MutableText.remote_deltas()`
    /// and splices remote edits into InputState via
    /// `replace_text_in_range_silent`.
    _remote_delta_subscription: Option<Task<()>>,
    /// Cancelled on drop. The `task_state` edge: the editable surface is a
    /// projection of BOTH columns, and the content subscription above is
    /// dropped for cell-attached editors, so this one carries the other half in
    /// either arm.
    _task_state_subscription: Option<Task<()>>,
    /// Bounds registry threaded from `GpuiRenderContext` so the popup
    /// overlay can register each item as a tracked widget. Lets the PBT
    /// driver observe the popup state via `wait_for_widget_kind` instead
    /// of poking the EditorViewModel directly.
    bounds_registry: BoundsRegistry,
    /// Persistent scroll state for the slash/link popup overlay. Kept on the
    /// view (not rebuilt per render) so `scroll_to_item` can keep the
    /// keyboard-selected entry in view and the user's manual scroll survives
    /// re-renders while the menu is open.
    popup_scroll: ScrollHandle,
    /// Last popup `selected_index` we programmatically scrolled into view.
    /// `scroll_to_item` must run ONLY when the keyboard selection actually
    /// moves — calling it every render (as the first cut of the 07-18 fix did)
    /// re-snaps the viewport to the selected row on every unrelated re-render
    /// (cursor blink, data-sync notify, signal ticks), which silently defeats
    /// the user's own mouse-wheel scroll: the menu appears to cap at the first
    /// screenful with "no scroll" (dogfood 2026-07-19). `None` while the popup
    /// is closed so the next open scrolls back to the top.
    popup_scrolled_index: std::cell::Cell<Option<usize>>,
    /// Last window-focus state observed by the render-path reconcile gate
    /// (`focus_transition`). Used to detect the frame where focus first arrives
    /// (false→true) so the NO-CELL builder backstop can re-sync a stale
    /// `InputState` from the live backend content *once*, before the user has
    /// typed — the backstop for a no-cell editor's data-sync subscription being
    /// orphaned by a row-set rebuild (split/join/navigation replaces the
    /// per-row `Mutable` cell). Cell-attached editors bypass this gate
    /// (Increment G).
    prev_focused: std::cell::Cell<bool>,
    /// Shared per-row cell, kept for the `task_state` column alone. The content
    /// subscription's handle is dropped for cell-attached editors (the CRDT
    /// owns content) while `task_state` has no second source — and without
    /// it the editable surface cannot render the block's vault syntax.
    task_row: Option<ReadOnlyMutable<Arc<DataRow>>>,
    /// The soft-keyboard focus generation this editor claimed on its last
    /// focus-gain (see `crate::soft_keyboard::editor_focus_gained`). Passed
    /// back on blur so a stale editor's late-arriving blur cannot hide the
    /// keyboard after a successor already claimed focus. Zero = never gained
    /// focus.
    focus_gen: std::cell::Cell<u64>,
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
        let context_params = std::collections::HashMap::from([(
            "id".into(),
            holon_api::Value::String(row_id.clone()),
        )]);
        let field_for_subscription = field.clone();
        let mut controller =
            EditorViewModel::new(operations, triggers, context_params, field, content.clone());
        controller.set_async_context(services.clone());
        // The keystroke sink is where an empty-born block stops being reapable.
        if let Some(newborns) = services.ephemeral_newborns() {
            controller.set_ephemeral_newborns(newborns);
        }
        // Attach a `Cell<String>` if the cell registry can resolve one.
        // Headless / stub / test paths leave it unattached and the VM's
        // pass-through CRDT methods become no-ops.
        // ALLOW(entity_uri_from_raw): boundary — `row_id` is the render-spec row id (a
        // `String`); parse once here before handing a typed URI to the cell registry.
        let row_uri = holon_api::EntityUri::from_raw(&row_id);
        if let Ok(cell) = services.editable_text(&row_uri, &field_for_subscription) {
            controller.attach_cell(cell);
        }

        // Increment G — seed `InputState` from the cell authority when a cell is
        // attached. `remote_deltas()` delivers only FUTURE deltas, so without
        // this seed a freshly-mounted cell-attached editor would display the
        // SQL-projected `content` prop until the first delta — the transient
        // staleness the removed just-focused render backstop used to cure. The
        // cell text is synchronously readable here (`current_text()` is a
        // synchronous borrow, already used below to seed the VM buffer).
        // Building `input` AFTER `attach_cell` means no stale value ever exists.
        // No cell (unwired / headless) → seed from `content`, exactly as before.
        let cell_attached = controller.has_cell();
        let seed_value = if cell_attached {
            controller.current_text().unwrap_or_default()
        } else {
            content.clone()
        };
        let controller = Arc::new(Mutex::new(controller));

        let input = cx.new(|cx| {
            let row_id_for_menu = row_id.clone();
            // Bare block UUID (no `block:` scheme) — the form org files store and
            // the form the user pastes into org refs / hands to agents. Parsed
            // once here at the render boundary.
            // ALLOW(entity_uri_from_raw): EditorView.row_id from render-spec node
            let bare_block_id = holon_api::EntityUri::from_raw(&row_id).id().to_string();
            InputState::new(window, cx)
                .auto_grow(1, usize::MAX)
                .default_value(&seed_value)
                .context_menu_extender(move |menu, _window, _cx| {
                    let row_id_for_click = row_id_for_menu.clone();
                    let bare_for_copy = bare_block_id.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Copy block ID").on_click(
                            move |_, _window, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    bare_for_copy.clone(),
                                ));
                                crate::share_ui::DegradedToastSink::push(
                                    crate::share_ui::DegradedToast {
                                        kind: crate::share_ui::DegradedKind::Info,
                                        shared_tree_id: bare_for_copy.clone(),
                                        detail: format!("Copied block ID {bare_for_copy}"),
                                        condition: None,
                                    },
                                    cx,
                                );
                            },
                        ))
                        .item(PopupMenuItem::new("Share subtree…").on_click(
                            move |_, _window, cx| {
                                ShareTrigger::trigger(row_id_for_click.clone(), cx);
                            },
                        ))
                })
        });

        // Kept for the `task_state` column: the editable surface shows a block's
        // vault syntax, and that column is what turns stored content into it.
        let task_row = data.clone();
        let field_for_task_state = field_for_subscription.clone();

        // Resolve the owning document's `#+TODO:` vocabulary ONCE for this
        // editing session and converge the surface when it lands. Until it does,
        // the surface shows stored content — safe by construction, because a
        // buffer that is not keyword-headed commits on the content channel and
        // can never change a task state.
        {
            let services_for_vocab = services.clone();
            // ALLOW(entity_uri_from_raw): render-spec row id (a `String`),
            // schemed once here before the vocabulary read.
            let block_for_vocab = holon_api::EntityUri::from_raw(&row_id);
            let ctrl_for_vocab = controller.clone();
            let field_for_vocab = field_for_subscription.clone();
            cx.spawn(async move |this, cx| {
                let vocabulary = holon_frontend::editor_source::vocabulary_for_block(
                    services_for_vocab.as_ref(),
                    &block_for_vocab,
                )
                .await;
                let vocabulary = match vocabulary {
                    Ok(Some(v)) => v,
                    // No query capability: nothing to read, so the surface stays
                    // UNCLASSIFIED (`Surface::Pending`) and its commits stay on
                    // the channel that cannot change a task state. Classifying
                    // under the parser's defaults instead would fabricate the
                    // one fact this whole seam exists to get right.
                    Ok(None) => {
                        tracing::debug!(
                            target: "editor.source_projection",
                            block = %block_for_vocab,
                            "no query capability; the editable surface stays the content column"
                        );
                        return;
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "editor.source_projection",
                            block = %block_for_vocab,
                            "cannot resolve this document's task-keyword vocabulary ({e:#}); the \
                             editable surface shows stored content and the keyword is not editable"
                        );
                        return;
                    }
                };
                ctrl_for_vocab
                    .lock()
                    .unwrap()
                    .set_task_vocabulary(vocabulary);
                let _ = cx.update(|cx| {
                    let Some(view) = this.upgrade() else {
                        return;
                    };
                    for window_handle in cx.windows() {
                        let _ = window_handle.update(cx, |_, window, cx| {
                            view.update(cx, |this, cx| {
                                // The RAW authority column, never the visible
                                // buffer: that one is already a projection, and
                                // projecting it again would restate the keyword.
                                let raw = match this.controller.lock().unwrap().current_text() {
                                    Some(cell) => Some(cell),
                                    None => this.task_row.as_ref().and_then(|row| {
                                        row.get_cloned()
                                            .get(field_for_vocab.as_str())
                                            .and_then(|v| v.as_string())
                                            .map(str::to_string)
                                    }),
                                };
                                let Some(raw) = raw else { return };
                                let target = this.project_authority(&raw);
                                this.converge_to("vocabulary_resolved", &target, window, cx);
                            });
                        });
                    }
                });
            })
            .detach();
        }

        // Subscribe to blur and change events.
        {
            let ctrl = controller.clone();
            let services_clone = services.clone();
            let row_id_for_blur = row_id.clone();
            // ALLOW(entity_uri_from_raw): render-spec row_id, schemed to match
            // the key an undo/redo arms its authority re-seed under.
            let row_uri_for_reseed = holon_api::EntityUri::from_raw(&row_id);
            cx.subscribe_in(
                &input,
                window,
                move |this, entity, event, window, cx| match event {
                    InputEvent::Focus => {
                        this.note_focus_gained();

                        // Promote this block to be the UiState.focused_block.
                        // Without this, clicking inside an editable_text gives the
                        // underlying Input gpui-focus but `focused_block` stays on
                        // whatever was focused before — chord keys and operations
                        // then dispatch against the wrong block. PBT inv-focus-matches-ref and the
                        // GeometryDriver read the focus from the engine's
                        // `focused_block_mutable()` Mutable, so this single write
                        // is the only update needed.
                        // ALLOW(entity_uri_from_raw): EditorView.row_id from render-spec
                        // node.row_id() (parsed on Focus/Blur)
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
                        // Focus edge: replay any directive deferred during a
                        // just-ended IME composition (Amendment 3).
                        this.replay_pending_directive(window, cx);
                        let _ = entity;
                    }
                    InputEvent::Blur => {
                        this.note_blur_event(entity, window, cx);

                        let value = entity.read(cx).value().to_string();
                        let action = ctrl.lock().unwrap().on_blur(&value);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);
                        // Blur edge: replay any IME-deferred directive.
                        this.replay_pending_directive(window, cx);
                        // Cursor position is no longer persisted on blur:
                        // editor focus + caret are pure
                        // in-memory UI state (ADR 0010),
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
                        // Route through the SAME applier the key handlers use:
                        // a keystroke can end a picker phase, and that carries
                        // text surgery `execute_action`'s no-window arm cannot
                        // do. Nothing here consumes a key — the change event
                        // has already happened.
                        let editor_id = this.input.entity_id();
                        let handle = window.window_handle();
                        let action = match apply_popup_action(
                            action,
                            &ctrl,
                            &entity.clone(),
                            &services_clone,
                            handle,
                            editor_id,
                            cx,
                        ) {
                            PopupActionOutcome::Handled => EditorAction::None,
                            PopupActionOutcome::NotPopup(action) => action,
                        };
                        execute_action(action, &services_clone, editor_id, cx);

                        // Local edit routes through the VM buffer — the write
                        // authority (buffer-ownership inversion). `apply_local_edit`
                        // mutates the authoritative buffer, and:
                        //   - cell mode: applies the delta through the CRDT;
                        //   - no-cell (SqlOnly) real block: stamps `write_seq`, records it as the
                        //     VM's `last_local_seq`, and returns the `set_field("content")` intent
                        //     the adapter dispatches (its sole commit funnel) so the typed text
                        //     lands before the next transition;
                        //   - creation placeholder / unchanged: returns `None`.
                        // The write_seq stamp happens INSIDE `apply_local_edit`
                        // before this dispatch, so a fast CDC echo cannot race a
                        // not-yet-recorded seq.
                        // A genuine keystroke makes this editor the authority
                        // again, so an armed undo re-seed must never reach it.
                        if ctrl.lock().unwrap().buffer() != text {
                            services_clone.consume_authority_reseed(&row_uri_for_reseed);
                        }
                        // The buffer is VAULT SYNTAX, so the VM routes the write
                        // to `source_text` when the text is (or has stopped
                        // being) keyword-headed and to `content` otherwise. The
                        // adapter does not need to know which — both are plain
                        // fire-and-forget `set_field`s, and the store's parse is
                        // the only thing that reads a keyword.
                        match ctrl.lock().unwrap().apply_local_edit(&text) {
                            Ok(Some(intent)) => services_clone.dispatch_intent(intent),
                            Ok(None) => {}
                            Err(e) => {
                                tracing::error!("apply_local_edit failed: {e}");
                            }
                        }

                        // A committed IME composition ends here with
                        // `ime_marked_range()` cleared — replay any deferred
                        // converge (discarded if this edit superseded it).
                        this.replay_pending_directive(window, cx);

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
        // 1. **Skip when focused.** While the user has the editor focused they are the
        //    source of truth — overwriting `InputState` while they're typing yanks the
        //    cursor to position 0 and drops the in-flight character. External changes
        //    during a focused edit are dropped from the *visible* state until the next
        //    focus cycle (data is still correct in the backend).
        //
        // 2. **Dedupe on the field's value.** The signal fires on every `.set()` of the
        //    per-row Mutable, including no-op writes triggered by unrelated field
        //    changes. `.dedupe_cloned()` on the extracted field value keeps the
        //    subscription quiet unless the relevant column actually changed.
        //
        // The render path no longer touches `set_value` — propagation is
        // entirely event-driven through this subscription. The returned
        // `Task<()>` cancels on drop, so removing this `EditorView`
        // (e.g. via collection driver `RemoveAt`) tears the subscription
        // down naturally.
        // Increment G — in cell-attached mode the entity `Cell`'s
        // `remote_deltas()` (subscribed below) is the SINGLE external content
        // source. The per-row `DataRow` subscription is bound to the
        // `ReactiveRowSet`'s per-row `Mutable`, which is replaced — orphaning the
        // subscription — on every split/join/nav rowset rebuild; that fragility
        // is the exact reason the render backstop existed. Drop the data handle
        // so `.map` never spawns it. The surviving cell subscription is
        // un-orphaned (`CellCache` returns the same live `Cell` for the same
        // block id across rebuilds), so no external update is lost. No-cell
        // (unwired / headless) editors keep the DataRow subscription.
        let data = if cell_attached { None } else { data };
        let _data_subscription: Option<Task<()>> = data.map(|data_handle| {
            let field_for_stream = field_for_subscription.clone();
            let signal = data_handle
                .signal_cloned()
                .map(move |row| {
                    let content = row
                        .get(&field_for_stream)
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string();
                    // Ordering token stamped by content writes
                    // (`holon_api::write_seq`), projected verbatim from
                    // `block_raw.write_seq`. `None` ONLY if the column is
                    // missing/mistyped — a plumbing regression the loop reports
                    // loudly and treats as "drop", never a silent converge.
                    let echo_seq = row.get("write_seq").and_then(|v| v.as_i64());
                    // `task_state` rides along ONLY so the dedupe below cannot
                    // swallow a task-toggle that leaves the content column
                    // alone: the surface is projected from both columns.
                    let task_state = row
                        .get("task_state")
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string();
                    (content, echo_seq, task_state)
                })
                .dedupe_cloned();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = signal.to_stream();
                // OP-VERSIONED ECHO SUPPRESSION (replaces the old
                // `user_idle`/`last_synced` heuristic, which mistook the moment
                // between two keystrokes for "idle" and therefore let a stale
                // echo overwrite in-flight typing — the vault-scale
                // "typing resets the block" P1). Each emission carries the
                // authority content AND its `write_seq` ordering token. We
                // converge only to a state at least as new as our last local
                // write; an older echo (a reordered/lagged CDC delivery of an
                // earlier keystroke) is dropped. A `split_block` truncation or
                // other structural mutation issued *after* the last keystroke
                // carries a greater-or-equal seq, so it still converges while
                // the editor owns focus — the property the old heuristic
                // existed to preserve.
                while let Some((new_value, echo_seq, _task_state)) = stream.next().await {
                    if this.upgrade().is_none() {
                        // EditorView dropped (e.g. row removed by
                        // collection driver). Stop the loop — the `Task`
                        // will be dropped shortly when our owning struct
                        // is freed, but exiting cleanly avoids a tight
                        // spin while the Drop runs.
                        break;
                    }
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        // Focus is window-scoped; pick the first window
                        // that owns this input entity. There is exactly
                        // one in normal app usage.
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    // The convergence DECISION lives in the VM
                                    // (`converge_from_data_sync`): it runs the
                                    // op-versioned echo-suppression rule against
                                    // the VM's own authoritative buffer, applies
                                    // the safe mid-composition mutations
                                    // (high-water advance / baseline adopt), and
                                    // returns a directive only when the visible
                                    // InputState must converge. The adapter
                                    // applies it now, or defers it past an IME
                                    // composition (Amendment 3).
                                    // The buffer holds vault syntax, so the
                                    // authority the echo discriminator compares
                                    // against is the PROJECTION of the settled
                                    // row — otherwise every source-channel write
                                    // reads back as an external change and
                                    // converges the keyword out of view.
                                    let surface = this.project_authority(&new_value);
                                    let directive = this
                                        .controller
                                        .lock()
                                        .unwrap()
                                        .converge_from_data_sync(&surface, echo_seq);
                                    if let Some(directive) = directive {
                                        this.converge_or_defer("data_sync", directive, window, cx);
                                    }
                                });
                            });
                        }
                    });
                }
            })
        });

        // The editable surface is projected from `content` AND `task_state`, and a
        // cell-attached editor drops the content subscription above (the CRDT
        // owns content) — so `task_state` gets its own edge, in both arms. It
        // carries no `write_seq` (no editor writes it), so it never feeds the
        // echo discriminator: it re-projects the live authority and converges.
        let _task_state_subscription: Option<Task<()>> = task_row.clone().map(|row| {
            let signal = row
                .signal_cloned()
                .map(|row| {
                    row.get("task_state")
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string()
                })
                .dedupe_cloned();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = signal.to_stream();
                while stream.next().await.is_some() {
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    let raw = match this.controller.lock().unwrap().current_text() {
                                        Some(cell) => Some(cell),
                                        None => this.task_row.as_ref().and_then(|row| {
                                            row.get_cloned()
                                                .get(field_for_task_state.as_str())
                                                .and_then(|v| v.as_string())
                                                .map(str::to_string)
                                        }),
                                    };
                                    let Some(raw) = raw else { return };
                                    let target = this.project_authority(&raw);
                                    this.converge_to("task_state", &target, window, cx);
                                });
                            });
                        }
                    });
                }
            })
        });

        // Window focus follows the in-memory `focused_block` authority
        // (ADR 0010): grab window focus whenever focus becomes this row.
        // Editor focus is never read back from Turso, so a late SQL
        // re-emission can't steal focus. Handles focus arriving at an
        // already-mounted (cache-reused) editor; the synchronous first-mount
        // grab below covers the fast path. RAII-scoped to this EditorView.
        // ALLOW(entity_uri_from_raw): render-spec row_id parsed once to match the focus
        // signal
        let row_uri_for_focus = holon_api::EntityUri::from_raw(&row_id);
        let _focus_subscription =
            spawn_focus_binding(cx, services.clone(), controller.clone(), row_uri_for_focus);

        // ── CRDT-backed remote delta subscription ──────
        //
        // When the view model has an attached `Cell<String>`, seed the
        // diff baseline from its current text and subscribe to remote
        // deltas. Cursor preservation uses Loro's `Cursor` anchoring via
        // the VM's `anchor_cursor` / `resolve_cursor` pass-throughs.
        let _ = field_for_subscription;
        let cell_for_remote = controller.lock().unwrap().cell().cloned();
        let _remote_delta_subscription: Option<Task<()>> = cell_for_remote.map(|cell| {
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = cell.remote_deltas();
                // Each remote delta is a WAKEUP only. The payload is discarded
                // STRUCTURALLY — `stream.next().await.is_some()` never binds the
                // `TextDelta`, so its `ops` are unreachable and cannot be applied
                // even by accident. We converge absolutely to `cell.current()`
                // (the authority) instead of replaying the delta. This structural
                // discard is a STRONGER guarantee than a runtime
                // `debug_assert!(ops.is_empty())` (the Step-0 alternative): it
                // holds in release builds and for EVERY backing — including the
                // SqlOnly wakeup-only `remote_deltas()` derived from `signal()`
                // (see `Cell::remote_deltas`), whose empty-`ops` deltas carry no
                // payload by construction. Out-of-order / coalesced deltas
                // therefore all land on the same string, and the prior
                // delta-replay sibling-flip (a stale splice landing on the wrong
                // block) is impossible.
                while stream.next().await.is_some() {
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    let input = this.input.clone();
                                    let (current, focused) = {
                                        let state = input.read(cx);
                                        (
                                            state.value().to_string(),
                                            state.focus_handle(cx).is_focused(window),
                                        )
                                    };
                                    // Focus/idle gate (mirrors the data path):
                                    // a focused, actively-typing editor keeps
                                    // its in-flight text; a focused-but-idle
                                    // editor — e.g. the just-focused merge
                                    // target after a join — DOES converge, and
                                    // that is how it receives the merged
                                    // content. `user_idle` := no unflushed
                                    // keystroke (VM buffer == InputState).
                                    let user_idle =
                                        this.controller.lock().unwrap().buffer() == current;
                                    if focused && !user_idle {
                                        return;
                                    }
                                    // Same VM directive path as the data-sync
                                    // loop: build the directive (target = the
                                    // live cell authority) and apply-or-defer it
                                    // behind the adapter-side IME guard
                                    // (Amendment 3). The structural payload of
                                    // the delta stays discarded above.
                                    let directive = this
                                        .controller
                                        .lock()
                                        .unwrap()
                                        .remote_converge_directive()
                                        .map(|d| ConvergeDirective {
                                            target: this.project_authority(&d.target),
                                            ..d
                                        });
                                    if let Some(directive) = directive {
                                        this.converge_or_defer(
                                            "remote_delta",
                                            directive,
                                            window,
                                            cx,
                                        );
                                    }
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
        // ALLOW(entity_uri_from_raw): render-spec row_id parsed vs focused_block() on
        // mount
        let row_uri = holon_api::EntityUri::from_raw(&row_id);
        if services.focused_block().as_ref() == Some(&row_uri) {
            grab_focus_and_seed_caret(
                &input,
                window,
                cx,
                services.as_ref(),
                &controller,
                &row_uri,
                true,
            );
        }

        Self {
            input,
            controller,
            row_id,
            services,
            nav,
            bounds_registry,
            popup_scroll: ScrollHandle::new(),
            popup_scrolled_index: std::cell::Cell::new(None),
            _data_subscription,
            _focus_subscription,
            _remote_delta_subscription,
            _task_state_subscription,
            prev_focused: std::cell::Cell::new(false),
            task_row,
            focus_gen: std::cell::Cell::new(0),
        }
    }

    /// Soft-keyboard focus hooks, keeping `focus_gen` in lockstep with the
    /// generation claimed on gain so blur can prove it is not stale.
    pub fn note_focus_gained(&self) {
        self.focus_gen
            .set(crate::soft_keyboard::editor_focus_gained());
    }

    /// A gpui focus-out event reached this editor's input. Routed through
    /// `editor_blur_event`, which re-reads the authoritative window focus:
    /// gpui also emits this when the focused element merely dropped out of
    /// the rendered frame or the window went inactive.
    pub fn note_blur_event(&self, input: &gpui::Entity<InputState>, window: &Window, cx: &mut App) {
        let focus = input.read(cx).focus_handle(cx);
        crate::soft_keyboard::editor_blur_event(window, &focus, cx, self.focus_gen.get());
    }

    /// The soft-keyboard focus generation this editor last claimed (0 if never
    /// focused). Callers that hold a live `entity.read(cx)` borrow — which
    /// blocks the `&mut cx` that `editor_focus_lost` needs — read this and pass
    /// it to `crate::soft_keyboard::editor_focus_lost` directly.
    pub fn focus_gen(&self) -> u64 {
        self.focus_gen.get()
    }

    /// Render-path focus-edge detector. Returns `(just_focused, just_blurred)`
    /// — the false→true and true→false window-focus transitions since the last
    /// call. On iOS/Android the gpui focus-change events never reach the
    /// editor's `InputEvent::Focus`/`Blur` subscription, so this render-path
    /// edge is the *only* reliable focus signal on mobile; the soft-keyboard
    /// raise/hide is driven from it (see `editable_text` builder).
    pub fn focus_transition(&self, is_focused: bool) -> (bool, bool) {
        let prev = self.prev_focused.get();
        let just_focused = is_focused && !prev;
        let just_blurred = !is_focused && prev;
        self.prev_focused.set(is_focused);
        (just_focused, just_blurred)
    }

    /// Apply a convergence `ConvergeDirective` now, or defer it on the view
    /// model when an IME composition is in progress (`ime_marked_range()` is
    /// `Some`). Deferred directives replay on the composition-end / focus edge
    /// via `replay_pending_directive` (Amendment 3) — a converge that lands
    /// mid-composition must never overwrite the in-flight composed text, but it
    /// also must not be silently dropped, or the buffer stays stale until an
    /// unrelated later echo.
    fn converge_or_defer(
        &mut self,
        source: &'static str,
        directive: ConvergeDirective,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input.read(cx).ime_marked_range().is_some() {
            self.controller
                .lock()
                .unwrap()
                .set_pending_directive(directive);
            return;
        }
        self.converge_to(source, &directive.target, window, cx);
    }

    /// Replay a directive deferred during an IME composition, once composition
    /// has ended (`ime_marked_range()` is `None`). No-op when nothing is
    /// pending or a newer local write superseded the deferred directive
    /// (see `EditorViewModel::take_pending_directive`). Called on the
    /// composition-end `InputEvent::Change` and on focus/blur edges of this
    /// editor.
    fn replay_pending_directive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.read(cx).ime_marked_range().is_some() {
            return; // still composing — wait for the end edge
        }
        let Some(directive) = self.controller.lock().unwrap().take_pending_directive() else {
            return;
        };
        self.converge_to("ime_replay", &directive.target, window, cx);
    }

    /// The single convergence entry point: set this editor's `InputState`
    /// from an external authority, absolutely and idempotently.
    ///
    /// Targets the Loro cell authority (`current_text()`) when a cell is
    /// attached, else `sql_default` (the SqlOnly DataRow content). Returns
    /// early when already in sync. Preserves the caret by anchoring it on the
    /// authority around the absolute `set_value` (which would otherwise force
    /// the caret to end); SqlOnly has no anchor, so a SqlOnly reconcile resets
    /// the caret to end — only ever reachable when the editor is unfocused or
    /// focus just arrived, never mid-typing.
    ///
    /// Syncs the VM `buffer` to the authority in lockstep (via
    /// `set_buffer_from_authority`) BEFORE the absolute `set_value`, so the
    /// re-entrant `InputEvent::Change` (gpui_component's "silent" splice is NOT
    /// silent — it emits Change unconditionally) sees `new_text == buffer` in
    /// `apply_local_edit` and writes nothing back. This is the "write only
    /// genuine user edits" invariant.
    ///
    /// `source` names the caller (`"remote_delta"`, `"data_sync"`,
    /// `"render_backstop"`, `"focus_reload"`) and is recorded on the
    /// `editor.converge_input` trace event emitted whenever this call
    /// actually mutates `InputState` (past the idempotent early-return).
    /// Gate 1 counts `source = "render_backstop"` events to prove the
    /// entity-`Cell` remote-delta path — not the backstop — carries
    /// cross-occurrence propagation.
    pub(crate) fn converge_input(
        &mut self,
        source: &'static str,
        sql_default: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = {
            let vm = self.controller.lock().unwrap();
            vm.current_text().unwrap_or_else(|| sql_default.to_string())
        };
        let target = self.project_authority(&raw);
        self.converge_to(source, &target, window, cx);
    }

    /// The block's stored state as the editable surface shows it: the ORG
    /// SOURCE its `(content, marks)` pair reconstructs, under the task-keyword
    /// projection. `raw` is the authority's CONTENT column — the stripped
    /// label — so the marks that turn it back into `~code~` / `[[u][Label]]`
    /// are read alongside it. Both they and the `task_state` come from the
    /// shared row cell, read now rather than remembered: a task toggle or a
    /// peer's mark edit under an open editor must change what the surface
    /// shows.
    fn project_authority(&self, raw: &str) -> String {
        let row = self.task_row.as_ref().map(|row| row.get_cloned());
        let task_state = row.as_ref().and_then(|row| {
            row.get("task_state")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        });
        let marks = row
            .as_ref()
            .map(|row| holon_frontend::link_segments::marks_of(row))
            .unwrap_or_default();
        self.controller
            .lock()
            .unwrap()
            .project_authority(raw, &marks, task_state.as_deref())
    }

    /// Set `InputState` to an editor-surface `target` that a caller has already
    /// projected. Split out of [`Self::converge_input`] so a directive built
    /// against the surface (the echo discriminator compares surface text) is
    /// never projected twice.
    pub(crate) fn converge_to(
        &mut self,
        source: &'static str,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = target.to_string();
        // Re-baseline the blur-commit change tracking to the authority we are
        // converging onto. `converge_input` is the single convergence entry
        // point and only fires at safe (non-mid-typing) points, so the buffer
        // is never a pending user edit here — it either already mirrors the
        // authority (early-return below) or is about to be re-seeded to it.
        // Without this, a re-seed leaves `original_value` at a stale
        // (mark-reconstructed) form and the NEXT blur diffs the re-seeded
        // (stripped) buffer as "changed", firing a spurious identical-content
        // `set_field("content")` that nulls live link marks and pollutes the
        // undo stack (BugFunnel 2026-07-13 defect (a)). Pure baseline update:
        // it dispatches nothing and does NOT advance `last_local_seq`.
        // Sync the VM buffer to the authority we are converging onto (the
        // write-authority buffer is also the self-echo sentinel: setting it
        // BEFORE `set_value` below makes the re-entrant Change a no-op, since
        // `apply_local_edit` sees `new_text == buffer`). Folds in the former
        // `rebaseline` — a pure baseline update that dispatches nothing and does
        // not advance the write-seq (passes the current high-water unchanged).
        {
            let mut vm = self.controller.lock().unwrap();
            let seq = vm.last_local_seq();
            vm.set_buffer_from_authority(&target, seq);
        }
        let input = self.input.clone();
        let current = input.read(cx).value().to_string();
        if current == target {
            return; // idempotent — nothing to converge
        }
        tracing::debug!(
            target: "editor.converge_input",
            source,
            row_id = %self.row_id,
            "converge InputState to authority"
        );
        // Capture the pre-set caret (UTF-8 byte offset) and anchor it on the
        // authority before the absolute set — the fork's `set_value` forces the
        // caret to text-end. The Loro-anchor path (cells only) is the refined
        // restore: it tracks concurrent edits. SqlOnly has no anchor
        // (`anchor_cursor` returns None), so we unconditionally restore
        // `prior_cursor` clamped to the new text — without it the first click
        // into an unfocused block converges here and jumps the caret to end.
        let prior_cursor = input.read(cx).cursor();
        let anchor = {
            let state = input.read(cx);
            let cursor_codepoint = state.text().offset_to_char_index(state.cursor());
            self.controller
                .lock()
                .unwrap()
                .anchor_cursor(cursor_codepoint, CursorBias::Left)
        };
        input.update(cx, |state, cx| {
            state.set_value(&target, window, cx);
        });
        let restored = anchor
            .as_ref()
            .and_then(|anchor| self.controller.lock().unwrap().resolve_cursor(anchor));
        // The Loro anchor resolves against the CELL text — the content column —
        // while the buffer holds the source projection, so an anchored offset
        // has to cross the same keyword prefix every other offset crosses.
        let anchor_prefix_chars = self
            .controller
            .lock()
            .unwrap()
            .current_text()
            .map(|cell| prepended_chars(&cell, &target))
            .unwrap_or(0);
        input.update(cx, |state, cx| {
            let byte_offset = match restored {
                Some(new_codepoint) => state
                    .text()
                    .char_index_to_offset(new_codepoint + anchor_prefix_chars),
                None => caret_after_converge(prior_cursor, &current, &target),
            };
            let pos = state.text().offset_to_position(byte_offset);
            state.set_cursor_position(pos, window, cx);
        });
        // The VM buffer was already synced to `target` above, so the deferred
        // re-entrant Change sees `new_text == buffer` → `apply_local_edit`
        // returns an empty edit → no spurious write-back.
    }
}

/// Clamp a caret byte offset captured before an absolute `set_value` onto the
/// new text: cap at its length and snap down to the nearest UTF-8 char boundary
/// so the restored caret is always a valid offset. The click position survives
/// convergence in SqlOnly mode (no Loro anchor); when the new text is shorter
/// the caret pins to the end.
/// Carry a caret across a converge that only PREPENDED bytes — which is what
/// seeding the source projection does: `milk` becomes `TODO milk`, every offset
/// in the text the user can see shifts right by the keyword prefix.
///
/// Any other shape (a genuine external rewrite) keeps the plain clamp: guessing
/// an edit script would move the caret on evidence the text does not carry.
fn caret_after_converge(prior: usize, old: &str, new: &str) -> usize {
    if new.len() > old.len() && new.ends_with(old) {
        return preserved_caret(prior + (new.len() - old.len()), new);
    }
    preserved_caret(prior, new)
}

/// How many CHARACTERS `new` prepends to `old`, `0` when it does not simply
/// prepend. Codepoints, not bytes: the Loro anchor speaks codepoint indices.
fn prepended_chars(old: &str, new: &str) -> usize {
    if new.len() > old.len() && new.ends_with(old) {
        return new.chars().count() - old.chars().count();
    }
    0
}

fn preserved_caret(old_offset: usize, new_text: &str) -> usize {
    let mut offset = old_offset.min(new_text.len());
    while !new_text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
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
    controller: Arc<Mutex<EditorViewModel>>,
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
                                &controller,
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
    let commit = ctrl.lock().unwrap().chord_commit_intent(live_text);
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
///   (matches the PBT ref's `model_chord_click_focus` and the headless mirror's
///   `seed_for_click`). Pre-mount keystrokes can't have placed a caret here:
///   blur-on-focus-leave drops them and the driver retries, so nothing
///   user-placed exists to be yanked.
/// - `default_caret_to_end = false` — the async focus subscription, which
///   re-fires on every focus arrival, *after* `home`+arrow keys may already
///   have moved the caret. An end-default there yanked the caret back to the
///   end, so `Enter` split at the end (source kept its full content, new block
///   empty — the SplitBlock-at-wrong-position bug). Leave an unseeded caret
///   alone.
///
/// `peek_caret_seed` is non-destructive, but this fn CONSUMES the seed after
/// applying it (see the tail). The seed is single-use: whichever of the sync
/// first-mount grab or the async focus-subscription runs first applies it and
/// clears it; the other sees no seed and leaves the placed caret alone. This is
/// what stops a later user click from re-applying a stale split/join offset.
fn grab_focus_and_seed_caret(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
    services: &dyn BuilderServices,
    controller: &Arc<Mutex<EditorViewModel>>,
    row: &holon_api::EntityUri,
    default_caret_to_end: bool,
) {
    if !input.read(cx).focus_handle(cx).is_focused(window) {
        window.focus(&input.read(cx).focus_handle(cx), cx);
    }
    // The seed arrives in CONTENT coordinates (`join_block` reports the merge
    // boundary, `split_block` reports 0) while this buffer holds the SURFACE, so
    // it crosses the keyword prefix on any tasked target (task #93).
    let seed = services.peek_caret_seed(row).and_then(|offset| {
        match controller.lock().unwrap().content_offset_to_surface(offset) {
            Ok(surface) => Some(surface),
            // The seed was armed against content this buffer no longer shows.
            // Leaving the caret where it is beats placing it somewhere the op
            // did not ask for, but it is a divergence, not a normal path.
            Err(e) => {
                tracing::error!(
                    target: "editor.caret_seed",
                    block = %row,
                    "caret seed {offset} dropped: {e}"
                );
                None
            }
        }
    });
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
    // Single-use: once applied, drop the seed so a LATER user click on this
    // same block derives its caret from the click position, not the stale
    // op-follow-up offset. Without this the split/join seed lingered in
    // `pending_caret_seed` (aged only by a focus MOVE to a different block) and
    // a re-click after a "failed click elsewhere" re-applied it, yanking the
    // caret to 0 → typing prepended (BugFunnel 2026-07-11 row 80). Consuming
    // here (not in the non-destructive `peek`) keeps the sync first-mount grab
    // and the async focus-subscription idempotent: whichever runs first applies
    // and clears; the other sees no seed and leaves the placed caret alone.
    if seed.is_some() {
        services.consume_caret_seed(row);
    }
}

impl EditorView {
    pub fn row_id(&self) -> &str {
        &self.row_id
    }

    pub fn input_entity(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Whether this editor has an attached content `Cell` (the stable-identity
    /// authority). Delegates to the same `EditorViewModel::has_cell()` the
    /// per-keystroke `InputEvent::Change` handler gates on, so "cell-attached"
    /// has exactly ONE definition across the write path and Increment G's
    /// sync-retirement gate (skip `_data_subscription` + the render backstop).
    pub fn has_cell(&self) -> bool {
        self.controller.lock().unwrap().has_cell()
    }

    /// Whether a popup overlay is open right now. Read from the controller, not
    /// from painted bounds: the bounds registry holds the LAST frame that
    /// recorded rows, so a closed menu still reads as open until something
    /// forces a repaint.
    pub fn is_popup_active(&self) -> bool {
        self.controller.lock().unwrap().is_popup_active()
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
        let window_handle = window.window_handle();
        // Everything a popup row's mouse handler needs to run its command
        // through the same path the Enter key uses.
        let click_target = PopupClickTarget {
            controller: self.controller.clone(),
            input: self.input.clone(),
            services: self.services.clone(),
            window_handle,
            editor_entity_id,
        };
        let popup_overlay = {
            let ctrl = self.controller.lock().unwrap();
            let max_h = popup_max_height_px(window.viewport_size().height.into());
            match ctrl.popup_state() {
                Some(s) => {
                    // Drive `scroll_to_item` ONLY on the frame the keyboard
                    // selection actually moved. Doing it every render re-snaps
                    // the viewport to the selected row on unrelated notifies and
                    // eats the user's mouse-wheel scroll (the "no scroll" cap).
                    let scroll_to_selection = popup_should_scroll_to_selection(
                        self.popup_scrolled_index.get(),
                        s.selected_index,
                    );
                    self.popup_scrolled_index.set(Some(s.selected_index));
                    Some(render_popup(
                        &s,
                        &self.bounds_registry,
                        &self.popup_scroll,
                        max_h,
                        scroll_to_selection,
                        &click_target,
                        cx,
                    ))
                }
                None => {
                    // Popup closed — forget the scrolled row so the next open
                    // starts at the top instead of a stale mid-list offset.
                    self.popup_scrolled_index.set(None);
                    None
                }
            }
        };

        let editor_entity = cx.entity();

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
                move |enter: &Enter, window, cx: &mut App| {
                    // Two layered guards keep Enter on a Page-level editor
                    // from acting on behalf of a focused child:
                    //   1. only the editor whose own InputState owns keyboard focus runs the
                    //      capture body, and
                    //   2. the operation targets `services.focused_block()` (UiState's notion of
                    //      focus), not this editor's own `row_id` — so even if the capture fires on
                    //      a shared/ancestor editor, the split lands on the logically focused leaf.
                    if !input.read(cx).focus_handle(cx).is_focused(window) {
                        return;
                    }
                    let target_id = services
                        .focused_block()
                        .map(|u| u.as_str().to_string())
                        .unwrap_or_else(|| row_id.clone());
                    // Cmd+Enter → cycle_task_state. `enter`, `shift-enter` and
                    // `secondary-enter` all resolve to this one action, so the
                    // chord is discriminated by the action's own `secondary`
                    // flag — the modifier GPUI parsed off THIS keystroke.
                    // `window.modifiers()` is ambient state maintained by
                    // separate ModifiersChanged events and does not describe
                    // the key that got us here.
                    //
                    // RESIDUAL for a platform lane: GPUI's `secondary` is cmd
                    // on macOS and ctrl elsewhere, while the structural
                    // registry publishes this chord as Cmd+Enter on every
                    // platform. On Linux/Windows ctrl+enter therefore cycles
                    // while the advertised chord does not.
                    if enter.secondary {
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
                    let action = match apply_popup_action(
                        action,
                        &ctrl,
                        &input,
                        &services,
                        window_handle,
                        editor_entity_id,
                        cx,
                    ) {
                        PopupActionOutcome::Handled => {
                            cx.stop_propagation();
                            return;
                        }
                        PopupActionOutcome::NotPopup(action) => action,
                    };
                    match action {
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
                            let live_text = input.read(cx).value().to_string();
                            if caret_probe() {
                                eprintln!(
                                    "[split-dispatch] target={target_id} cursor={cursor_byte} \
                                     editor_text={live_text:?}"
                                );
                            }
                            // The caret is measured on the vault syntax the
                            // widget shows; the split cuts the content column
                            // under it. Crossing that seam is the VM's job and
                            // it can REFUSE — a caret that does not land on the
                            // buffer it was measured on is a routing bug, and
                            // splitting somewhere else would silently rewrite
                            // the user's text.
                            let caret = match ctrl
                                .lock()
                                .unwrap()
                                .structural_caret(&live_text, cursor_byte)
                            {
                                Ok(caret) => caret,
                                Err(e) => {
                                    tracing::error!(
                                        target: "editor.split",
                                        block = %target_id,
                                        "Enter dropped: {e}"
                                    );
                                    cx.stop_propagation();
                                    return;
                                }
                            };
                            if let Some(intent) =
                                structural_block_action(EditorKey::Enter, &target_id, caret)
                            {
                                dispatch_structural_as_commit_point(
                                    &ctrl, &services, &live_text, intent,
                                );
                            }
                            cx.stop_propagation();
                        }
                        EditorAction::Propagate => {
                            cx.propagate();
                        }
                        // `apply_popup_action` hands back only `None` and
                        // `Propagate`; every popup outcome was applied there.
                        _ => cx.propagate(),
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                let input = self.input.clone();
                let services = self.services.clone();
                move |_: &Escape, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Escape);
                    // Escape out of a picker phase carries the command text
                    // back into the block, so it needs the same text surgery
                    // the other popup outcomes do.
                    if let PopupActionOutcome::NotPopup(action) = apply_popup_action(
                        action,
                        &ctrl,
                        &input,
                        &services,
                        window_handle,
                        editor_entity_id,
                        cx,
                    ) {
                        if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                    } else {
                        // The Escape cancelled a picker phase; it is spent.
                        cx.stop_propagation();
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
                    // A picker phase whose command text is hidden owns the
                    // backspace at its anchor: it cancels the phase and puts the
                    // text back. Letting the delete run first would eat into the
                    // very region the anchor addresses, which is how a hide-time
                    // offset ends up indexing past the end of the buffer.
                    let text = input.read(cx).value().to_string();
                    let (line_start, line_end) = line_bounds(&text, cursor_byte);
                    let cancel = ctrl.lock().unwrap().cancel_hidden_phase_at_anchor(
                        &text[line_start..line_end],
                        cursor_byte - line_start,
                    );
                    if let Some(action) = cancel {
                        apply_popup_action(
                            action,
                            &ctrl,
                            &input,
                            &services,
                            window_handle,
                            editor_entity_id,
                            cx,
                        );
                        cx.stop_propagation();
                        return;
                    }
                    if cursor_byte != 0 {
                        // Not at start — let InputState handle char delete.
                        return;
                    }
                    // Backspace-at-0 → join. Decision shared with the headless
                    // mirror via `structural_block_action`.
                    if let Some(intent) = structural_block_action(
                        EditorKey::Backspace,
                        &row_id,
                        StructuralCaret::on_plain_text(0),
                    ) {
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
                    if let Some(intent) = structural_block_action(
                        EditorKey::Tab,
                        &row_id,
                        StructuralCaret::on_plain_text(0),
                    ) {
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
                    if let Some(intent) = structural_block_action(
                        EditorKey::BackTab,
                        &row_id,
                        StructuralCaret::on_plain_text(0),
                    ) {
                        let live_text = input.read(cx).value().to_string();
                        // ADR 0028 D1: outdent of a direct page child is REJECTED
                        // by the op engine (it would escape the page container).
                        // Route through the awaitable path — same as the slash-menu
                        // ops (commit abfbed92) — so the rejection surfaces as a
                        // visible CommandFailed toast, not just a `tracing::error!`.
                        // Commit any pending text edit FIRST (structural commit
                        // point), then dispatch outdent and await its result.
                        let commit = ctrl.lock().unwrap().chord_commit_intent(&live_text);
                        let services_for = services.clone();
                        let rt = services.runtime_handle();
                        cx.spawn(async move |cx| {
                            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
                            rt.spawn(async move {
                                if let Some(c) = commit {
                                    let _ = services_for.dispatch_intent_awaitable(c).await;
                                }
                                let outcome = services_for
                                    .dispatch_intent_awaitable(intent)
                                    .await
                                    .err()
                                    .map(|e| format!("{e:#}"));
                                let _ = tx.send(outcome);
                            });
                            if let Ok(Some(detail)) = rx.await {
                                let _ = cx.update_window(window_handle, |_, _window, cx| {
                                    crate::share_ui::DegradedToastSink::push(
                                        crate::share_ui::DegradedToast {
                                            kind: crate::share_ui::DegradedKind::CommandFailed,
                                            shared_tree_id: "command".into(),
                                            detail,
                                            condition: None,
                                        },
                                        cx,
                                    );
                                });
                            }
                        })
                        .detach();
                    }
                    cx.stop_propagation();
                }
            })
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                let input = self.input.clone();
                let ctrl = self.controller.clone();
                move |_: &crate::TurnIntoPage, _window, cx: &mut App| {
                    // Same op the slash-menu "Turn into page" entry runs
                    // (`execute_operation` with the `convert_block_to_page`
                    // descriptor). The descriptor maps `id -> target`; here we
                    // supply `target` directly (the focused block id). Commit
                    // any pending edit first (structural commit point) so the
                    // planner reads up-to-date content.
                    let mut params = std::collections::HashMap::new();
                    params.insert(
                        "target".to_string(),
                        holon_api::Value::String(row_id.clone()),
                    );
                    let intent = holon_frontend::operations::OperationIntent::new(
                        holon_api::EntityName::new("block"),
                        "convert_block_to_page".to_string(),
                        params,
                    );
                    // Registry name for the chord, beside the intent it
                    // causes: a reply matching on `turn_into_page` would
                    // otherwise see only `convert_block_to_page` and call the
                    // press a different action. Settled Ok because dispatching
                    // IS this handler's job — the intent's own journal entry
                    // carries the operation's outcome.
                    if let Some(journal) = services.dispatch_journal() {
                        let seq = journal.record_window_action("turn_into_page");
                        journal.settle(seq, Ok(()));
                    }
                    let live_text = input.read(cx).value().to_string();
                    dispatch_structural_as_commit_point(&ctrl, &services, &live_text, intent);
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
                "cross_block_nav: bubble_input returned None for row_id={row_id}, \
                 direction={direction:?} (router={})",
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
/// Preferred content height of the slash/link popup, in px, when the window
/// is tall enough (see [`POPUP_MARGIN_PX`]).
const POPUP_DESIRED_HEIGHT_PX: f32 = 240.0;
/// Smallest content height we will ever cap the popup to. A window shorter
/// than this shows a popup that overruns it, but that is degenerate (the popup
/// is still scrollable and snaps into the window via `anchored`).
const POPUP_MIN_HEIGHT_PX: f32 = 48.0;
/// Gap kept between the popup and the window edge so it never sits flush.
const POPUP_MARGIN_PX: f32 = 16.0;

/// Cap the popup's max content height so it fits within the window viewport.
///
/// The popup opens below (or, via `anchored`, above) the caret and scrolls
/// internally once entries exceed this height. Bounding the height to the
/// viewport is what stops the menu from running past the window bottom with
/// its lower entries unreachable — the reported truncation bug.
///
/// Pure so the height policy is unit-tested without a live gpui window.
fn popup_max_height_px(viewport_height: f32) -> f32 {
    (viewport_height - POPUP_MARGIN_PX).clamp(POPUP_MIN_HEIGHT_PX, POPUP_DESIRED_HEIGHT_PX)
}

/// Whether this render should re-drive `scroll_to_item` to reveal the
/// keyboard-selected popup row.
///
/// Returns true only when the selection has actually MOVED since the last
/// programmatic scroll. Scrolling every render re-snaps the viewport to the
/// selected row on unrelated re-renders (cursor blink, data-sync notify,
/// signal ticks), which cancels the user's own mouse-wheel scroll and makes a
/// long menu look capped with "no scroll" (dogfood 2026-07-19). `prev` is
/// `None` while the popup is closed, so the first frame of a fresh open always
/// scrolls (to the top, index 0).
///
/// Pure so the scroll-gating policy is unit-tested without a live gpui window.
fn popup_should_scroll_to_selection(prev: Option<usize>, selected: usize) -> bool {
    prev != Some(selected)
}

/// Byte range of the line `cursor` sits on, as `(start, end)` — end exclusive
/// of the newline. Slash-command spans are line-relative, so every arm that
/// edits one needs both edges to stay inside the line it was typed on.
fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let end = text[start..]
        .find('\n')
        .map(|p| start + p)
        .unwrap_or(text.len());
    (start, end)
}

/// What [`apply_popup_action`] did with an `EditorAction`.
enum PopupActionOutcome {
    /// The action was a popup outcome and has been fully applied.
    Handled,
    /// Not applied here (`None`, `Propagate`, `PopupActivated`). Only the
    /// caller knows what the gesture means with no menu open, and only the
    /// change handler can spawn a popup's item-signal watcher.
    NotPopup(EditorAction),
}

/// Apply a popup-originated `EditorAction` to the live editor: the text
/// surgery, the dispatch, and the toast.
///
/// Shared by the Enter key handler, the popup row click handler, the Escape
/// handler and the per-keystroke change handler so all four do byte-identical
/// work (task #45). Forking a second dispatch path here is what left every
/// command mouse-dead in the first place.
///
/// Applies editor state only — CONSUMING the gesture is the caller's call,
/// since the change handler has no key to consume.
fn apply_popup_action(
    action: EditorAction,
    controller: &Arc<Mutex<EditorViewModel>>,
    input: &Entity<InputState>,
    services: &Arc<dyn BuilderServices>,
    window_handle: gpui::AnyWindowHandle,
    editor_entity_id: gpui::EntityId,
    cx: &mut App,
) -> PopupActionOutcome {
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

            let mut new_text = String::with_capacity(text.len() + replacement.len());
            new_text.push_str(&text[..abs_start]);
            new_text.push_str(&replacement);
            new_text.push_str(&text[cursor..]);
            let new_cursor_offset = abs_start + replacement.len();

            let input = input.clone();
            cx.spawn(async move |cx| {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    input.update(cx, |state, cx| {
                        state.set_value(&new_text, window, cx);
                        let pos = state.text().offset_to_position(new_cursor_offset);
                        state.set_cursor_position(pos, window, cx);
                    });
                });
            })
            .detach();
            cx.notify(editor_entity_id);
        }
        EditorAction::Execute(intent) => {
            services.dispatch_intent(intent);
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
            let services_for_dispatch = services.clone();
            let rt = services.runtime_handle();
            // ONE ordered spawn: (B) strip the typed "/command"
            // from the editor FIRST, THEN dispatch. Previously
            // the strip ran in a SEPARATE detached spawn that
            // raced the synchronous `dispatch_intent`, so the
            // menu-trigger `/` was still in the origin's content
            // when `convert_block_to_page` read it (GPUI dogfood
            // 2026-07-20, bug a2). Sequencing strip→dispatch in
            // one future removes that race.
            //
            // (D) EVERY menu-dispatched op now goes through the
            // awaitable path so a backend failure surfaces as a
            // visible toast — fail-loud, not a lone
            // `tracing::error!`. Before, only
            // `instantiate_template` got the toast; convert (and
            // every other slash op) failed silently (bug a3).
            cx.spawn(async move |cx| {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    input.update(cx, |state, cx| {
                        state.set_value(&new_text, window, cx);
                        let pos = state.text().offset_to_position(abs_start);
                        state.set_cursor_position(pos, window, cx);
                    });
                });
                let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
                rt.spawn(async move {
                    let outcome = services_for_dispatch
                        .dispatch_intent_awaitable(intent)
                        .await
                        .err()
                        .map(|e| format!("{e:#}"));
                    let _ = tx.send(outcome);
                });
                if let Ok(Some(detail)) = rx.await {
                    let _ = cx.update_window(window_handle, |_, _window, cx| {
                        crate::share_ui::DegradedToastSink::push(
                            crate::share_ui::DegradedToast {
                                kind: crate::share_ui::DegradedKind::CommandFailed,
                                shared_tree_id: "command".into(),
                                detail,
                                condition: None,
                            },
                            cx,
                        );
                    });
                }
            })
            .detach();
            cx.notify(editor_entity_id);
        }
        EditorAction::CommandFailed {
            message,
            strip_prefix_start,
        } => {
            // A menu selection was handled but failed. Fail-loud:
            // (1) strip the typed "/command" text (same span
            // arithmetic as ExecuteAndStripCommand), (2) surface a
            // visible toast, (3) consume the Enter so it does NOT
            // fall through to split_block (the selection already
            // consumed the key — a stray split would be silent
            // corruption). This is the fix for the live-drive
            // regression where a failed template insert split the
            // block instead of reporting the failure.
            if let Some(strip_prefix_start) = strip_prefix_start {
                let text = input.read(cx).value().to_string();
                let cursor = input.read(cx).cursor();
                let line_start = text[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let abs_start = line_start + strip_prefix_start;
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
            }
            crate::share_ui::DegradedToastSink::push(
                crate::share_ui::DegradedToast {
                    kind: crate::share_ui::DegradedKind::CommandFailed,
                    shared_tree_id: "command".into(),
                    detail: message,
                    condition: None,
                },
                cx,
            );
            cx.notify(editor_entity_id);
        }
        EditorAction::HideCommandText { prefix_start, len } => {
            // Lift the typed command out of the visible block for as long as
            // the picker phase lasts (ruling D1.b) and hand it, plus the text
            // that stood before it, to the controller — which owns putting it
            // back on a cancel.
            let text = input.read(cx).value().to_string();
            let cursor = input.read(cx).cursor();
            let (line_start, line_end) = line_bounds(&text, cursor);
            let abs_start = line_start + prefix_start;
            let abs_end = abs_start + len;
            if abs_end > line_end
                || !text.is_char_boundary(abs_start)
                || !text.is_char_boundary(abs_end)
            {
                // The menu's idea of the command no longer fits the line it was
                // typed on. Leave the text visible rather than cutting a span
                // that means nothing — degraded, but disclosed and not corrupt.
                tracing::error!(
                    abs_start,
                    abs_end,
                    line_end,
                    "slash-command hide span does not fit its line; leaving the command text visible"
                );
                cx.notify(editor_entity_id);
                return PopupActionOutcome::Handled;
            }
            let hidden = text[abs_start..abs_end].to_string();
            let line_prefix = text[line_start..abs_start].to_string();
            let mut new_text = String::with_capacity(text.len());
            new_text.push_str(&text[..abs_start]);
            new_text.push_str(&text[abs_end..]);
            controller
                .lock()
                .unwrap()
                .command_text_hidden(line_prefix, hidden);

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
            cx.notify(editor_entity_id);
        }
        EditorAction::RestoreCommandText {
            line_prefix,
            text: restored,
        } => {
            // The picker was cancelled: put the command text back verbatim, in
            // front of whatever search term the user had typed.
            //
            // The anchor is RE-DERIVED from the live line rather than taken as
            // a hide-time offset. An offset captured when the text was hidden
            // stops addressing the same place the moment the line changes under
            // it, and slicing with it either panics or reinserts the command
            // somewhere the user never typed it.
            let text = input.read(cx).value().to_string();
            let cursor = input.read(cx).cursor();
            let (line_start, line_end) = line_bounds(&text, cursor);
            if !text[line_start..line_end].starts_with(line_prefix.as_str()) {
                tracing::warn!(
                    line_prefix = %line_prefix,
                    restored = %restored,
                    "the line no longer starts with the hidden command's prefix; not restoring it"
                );
                cx.notify(editor_entity_id);
                return PopupActionOutcome::Handled;
            }
            let abs_start = line_start + line_prefix.len();
            let mut new_text = String::with_capacity(text.len() + restored.len());
            new_text.push_str(&text[..abs_start]);
            new_text.push_str(&restored);
            new_text.push_str(&text[abs_start..]);
            let new_cursor = cursor.max(abs_start) + restored.len();
            let restored_line = new_text[line_start..line_end + restored.len()].to_string();
            controller
                .lock()
                .unwrap()
                .command_text_restored(restored_line);

            let input = input.clone();
            cx.spawn(async move |cx| {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    input.update(cx, |state, cx| {
                        state.set_value(&new_text, window, cx);
                        let pos = state.text().offset_to_position(new_cursor);
                        state.set_cursor_position(pos, window, cx);
                    });
                });
            })
            .detach();
            cx.notify(editor_entity_id);
        }
        EditorAction::PopupDismissed | EditorAction::UpdatePopup => {
            cx.notify(editor_entity_id);
        }
        // `PopupActivated` is a SUBSCRIPTION to set up, not editor text to
        // edit, and only the change handler's context can spawn the item-signal
        // watcher. Swallowing it here left the menu permanently empty.
        action @ (EditorAction::None
        | EditorAction::Propagate
        | EditorAction::PopupActivated { .. }) => {
            return PopupActionOutcome::NotPopup(action);
        }
    }
    PopupActionOutcome::Handled
}

/// Everything a popup row's mouse handler needs to run the picked command.
/// Cloned per render pass and moved into each row's `on_mouse_down`.
#[derive(Clone)]
struct PopupClickTarget {
    controller: Arc<Mutex<EditorViewModel>>,
    input: Entity<InputState>,
    services: Arc<dyn BuilderServices>,
    window_handle: gpui::AnyWindowHandle,
    editor_entity_id: gpui::EntityId,
}

fn render_popup(
    state: &PopupState,
    bounds_registry: &BoundsRegistry,
    scroll: &ScrollHandle,
    max_height_px: f32,
    scroll_to_selection: bool,
    click_target: &PopupClickTarget,
    cx: &App,
) -> Deferred {
    use gpui::div;
    use gpui::prelude::*;
    use gpui::px;
    use gpui_component::theme::ActiveTheme;

    let theme = cx.theme().colors;
    let bg = theme.popover;
    let border = theme.border;
    let text_color = theme.foreground;
    let selected_bg = theme.accent;
    let selected_text = theme.accent_foreground;
    let muted = theme.muted_foreground;

    let mut container = div()
        .id("popup-scroll")
        .w(px(280.0))
        .max_h(px(max_height_px))
        .overflow_y_scroll()
        .track_scroll(scroll)
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
                .when(is_selected, |d| d.bg(selected_bg).text_color(selected_text))
                // A pointer pick runs the SAME `select_current` an Enter on
                // this row would (task #45: the rows carried no handler at
                // all, so every slash command was mouse-dead). Mouse-DOWN,
                // not click: the press must beat the editor's own focus
                // handling, and the menu is gone by the time the button
                // comes back up.
                .on_mouse_down(gpui::MouseButton::Left, {
                    let target = click_target.clone();
                    // The id this row PAINTED. Items refill asynchronously, so
                    // the index alone can address a different command by the
                    // time the click lands; the controller runs the row only
                    // while the index still holds this id.
                    let clicked_id = item.id.clone();
                    move |_event, _window, cx: &mut App| {
                        let action = target
                            .controller
                            .lock()
                            .unwrap()
                            .on_popup_item_clicked(i, &clicked_id);
                        apply_popup_action(
                            action,
                            &target.controller,
                            &target.input,
                            &target.services,
                            target.window_handle,
                            target.editor_entity_id,
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify(target.editor_entity_id);
                    }
                });

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

    // Keep the keyboard-selected entry inside the scroll viewport as the user
    // arrows through a list longer than `max_height_px`. Gated on an actual
    // selection change (see `EditorView.popup_scrolled_index`) so unrelated
    // re-renders don't re-snap the viewport and eat the user's mouse-wheel
    // scroll — the "caps at one screenful, no scroll" dogfood bug.
    if scroll_to_selection {
        scroll.scroll_to_item(state.selected_index);
    }

    // `anchored` positions the popup one line below the caret and, when that
    // would overrun the window bottom, flips it above / snaps it back inside
    // the viewport (default `SwitchAnchor` fit) — so lower entries are never
    // clipped off-screen and unreachable.
    deferred(
        anchored()
            .anchor(Corner::TopLeft)
            .offset(point(px(0.0), px(20.0)))
            .child(container),
    )
    .with_priority(1)
}

/// Save clipboard image bytes to the org attachments directory.
/// Returns the relative path (e.g. "attachments/a1b2c3d4.png").
fn save_clipboard_image(bytes: &[u8], extension: &str) -> Result<String, std::io::Error> {
    let root = org_root_dir();
    let attachments = root.join("attachments");
    std::fs::create_dir_all(&attachments)?;

    use std::hash::Hash;
    use std::hash::Hasher;
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
    if let Ok(root) = std::env::var("HOLON_VAULT_ROOT") {
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

/// Execute an EditorAction in a context without window access (subscribe
/// callbacks).
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

#[cfg(test)]
mod popup_layout {
    //! Height policy for the slash/link popup overlay. The layout/scroll wiring
    //! itself (`overflow_y_scroll` + `track_scroll` + `anchored` flip) needs a
    //! live gpui window to exercise and is structurally invisible to the
    //! headless keystone PBT — see docs/Testing/BugFunnel.md. What *is* pure
    //! and testable is the cap that stops the menu running past the window
    //! bottom.

    use super::POPUP_DESIRED_HEIGHT_PX;
    use super::POPUP_MARGIN_PX;
    use super::POPUP_MIN_HEIGHT_PX;
    use super::popup_max_height_px;
    use super::popup_should_scroll_to_selection;

    #[test]
    fn fresh_open_scrolls_to_top() {
        // Popup closed (`None`) → first frame scrolls (to index 0).
        assert!(popup_should_scroll_to_selection(None, 0));
    }

    #[test]
    fn unchanged_selection_does_not_rescroll() {
        // The regression: an unrelated re-render (same selection) must NOT
        // re-drive scroll_to_item, or it cancels the user's mouse-wheel scroll
        // and the menu looks capped with no scroll.
        assert!(!popup_should_scroll_to_selection(Some(3), 3));
    }

    #[test]
    fn moved_selection_scrolls_into_view() {
        // Arrowing down to a new row re-drives scroll-into-view.
        assert!(popup_should_scroll_to_selection(Some(3), 4));
        assert!(popup_should_scroll_to_selection(Some(4), 3));
    }

    #[test]
    fn tall_window_caps_at_desired_height() {
        // A roomy window must not let the popup grow unbounded; it caps at the
        // preferred height and scrolls internally beyond that.
        assert_eq!(popup_max_height_px(1000.0), POPUP_DESIRED_HEIGHT_PX);
    }

    #[test]
    fn short_window_shrinks_to_fit_minus_margin() {
        // The regression: a window shorter than the desired height must shrink
        // the popup to the available space (leaving a margin) instead of
        // overrunning the bottom edge with unreachable entries.
        let vh = 150.0;
        assert_eq!(popup_max_height_px(vh), vh - POPUP_MARGIN_PX);
        assert!(popup_max_height_px(vh) < POPUP_DESIRED_HEIGHT_PX);
    }

    #[test]
    fn degenerate_tiny_window_floors_at_min() {
        // Below the floor we stop shrinking (a popup with no usable rows is
        // useless); `anchored` still snaps it into the window.
        assert_eq!(popup_max_height_px(10.0), POPUP_MIN_HEIGHT_PX);
    }
}

#[cfg(test)]
mod caret_preservation {
    //! Directed regression tests for [`preserved_caret`] — the clamp that keeps
    //! a click-placed caret from jumping to text-end when `converge_input`
    //! re-seeds an unfocused editor's `InputState` in SqlOnly mode (no Loro
    //! anchor). BugFunnel: first click into an unfocused block placed the caret
    //! at end instead of the clicked char.

    use super::preserved_caret;

    #[test]
    fn mid_text_offset_is_preserved() {
        // The clicked byte offset lands inside the unchanged converged text.
        assert_eq!(preserved_caret(3, "hello world"), 3);
    }

    #[test]
    fn offset_past_end_pins_to_length() {
        // New (converged) text is shorter than where the caret was — pin to end
        // rather than produce an out-of-bounds offset.
        assert_eq!(preserved_caret(20, "hello"), 5);
    }

    #[test]
    fn offset_equal_to_length_is_end() {
        assert_eq!(preserved_caret(5, "hello"), 5);
    }

    #[test]
    fn zero_offset_stays_at_start() {
        assert_eq!(preserved_caret(0, "hello"), 0);
    }

    #[test]
    fn multibyte_boundary_snaps_down() {
        // "café" — 'é' is 2 bytes (indices 3..5); an offset landing mid-'é' (4)
        // must snap down to the char boundary 3, never a raw byte min that would
        // panic in `offset_to_position`.
        let text = "café";
        assert_eq!(text.len(), 5);
        assert_eq!(preserved_caret(4, text), 3);
    }

    #[test]
    fn multibyte_valid_boundary_is_kept() {
        // An offset already on a char boundary of a multibyte string is kept.
        assert_eq!(preserved_caret(3, "café"), 3);
    }
}

#[cfg(test)]
mod source_projection_caret {
    //! Inc 2 — focus seeds the SOURCE PROJECTION, which grows the buffer at the
    //! FRONT by the task keyword. Every offset the user placed against the
    //! displayed text has to cross that prefix, or a mid-word click lands
    //! `keyword.len() + 1` bytes to the left of the character it was aimed at.
    use super::caret_after_converge;
    use super::prepended_chars;

    #[test]
    fn a_mid_word_caret_follows_the_keyword_prefix() {
        // Clicked between `mi` and `lk` of the displayed `milk`, then focus
        // seeds `TODO milk`: the caret must still sit between `mi` and `lk`.
        assert_eq!(caret_after_converge(2, "milk", "TODO milk"), 7);
    }

    #[test]
    fn a_caret_at_the_start_of_the_content_stays_at_its_start() {
        assert_eq!(caret_after_converge(0, "milk", "TODO milk"), 5);
        assert_eq!(caret_after_converge(4, "milk", "TODO milk"), 9);
    }

    #[test]
    fn a_genuine_external_rewrite_keeps_the_plain_clamp() {
        // Not a prepend — the text was replaced, and there is no evidence for
        // where the caret "should" go, so it clamps as before.
        assert_eq!(caret_after_converge(3, "milk", "bread"), 3);
        assert_eq!(caret_after_converge(9, "milk", "oat"), 3);
    }

    #[test]
    fn an_unprojected_converge_does_not_move_the_caret() {
        assert_eq!(caret_after_converge(2, "milk", "milk"), 2);
    }

    #[test]
    fn the_prefix_is_counted_in_codepoints_for_the_loro_anchor() {
        // The anchor speaks codepoint indices, so a multibyte keyword must not
        // shift it by its BYTE length.
        assert_eq!(prepended_chars("milk", "TODO milk"), 5);
        assert_eq!(prepended_chars("milk", "TÄT milk"), 4);
        assert_eq!(prepended_chars("milk", "bread"), 0);
        assert_eq!(prepended_chars("milk", "milk"), 0);
    }

    #[test]
    fn a_selection_endpoint_crosses_the_prefix_the_same_way() {
        // Anchor and head are two offsets in the same coordinate space, so the
        // one rule carries both — a selection over the whole displayed text
        // still selects exactly that text, not the keyword with it.
        let (anchor, head) = (0usize, 4usize);
        assert_eq!(caret_after_converge(anchor, "milk", "TODO milk"), 5);
        assert_eq!(caret_after_converge(head, "milk", "TODO milk"), 9);
    }
}
