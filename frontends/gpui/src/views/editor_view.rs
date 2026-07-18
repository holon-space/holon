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
use holon_frontend::RowOrigin;
use holon_frontend::cell::CursorBias;
use holon_frontend::cell::compute_text_delta;
use holon_frontend::editor_view_model::EditorAction;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::EditorViewModel;
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

/// Outcome of applying the op-versioned echo-suppression rule to one data-sync
/// emission. Pure and side-effect-free so the convergence policy is unit-tested
/// directly (see the `echo_suppression` tests) without a live gpui window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EchoDecision {
    /// Echo equals the editor's current InputState — nothing to change. If the
    /// echo carried a sequence, advance the high-water mark to it so a later
    /// reordered echo of an even earlier keystroke is still recognised as
    /// stale.
    InSync { advance_to: Option<i64> },
    /// Converge InputState to the echo and adopt `seq` as the new high-water.
    Converge { seq: i64 },
    /// A reordered/lagged echo of an edit strictly older than the editor's last
    /// local write. Drop it — this is the "typing resets the block" fix.
    DropStale,
    /// Content changed but the row carried no `write_seq` ordering token — a
    /// schema/projection regression. Drop and report loudly (never converge
    /// blindly: that is the stale-echo data loss we are preventing).
    DropNoSeq,
}

/// Op-versioned echo suppression for the SqlOnly data-sync path.
///
/// Converge to an authority state only when it is **at least as new** as the
/// editor's last local write (`echo_seq >= last_local_seq`). A stale/reordered
/// echo of an earlier keystroke (`echo_seq < last_local_seq`) is dropped; a
/// `split_block` truncation or peer edit issued after the last keystroke
/// carries a greater-or-equal seq and still converges. Content equality
/// short-circuits (the editor's own latest echo, or a redundant emit). Ordering
/// — not content — is authoritative because the dispatcher's inline-mark
/// stripping rewrites the stored value, so an editor's own echo legitimately
/// differs from what it typed.
pub(crate) fn evaluate_data_sync_echo(
    current: &str,
    new_value: &str,
    echo_seq: Option<i64>,
    last_local_seq: i64,
) -> EchoDecision {
    if current == new_value {
        return EchoDecision::InSync {
            advance_to: echo_seq,
        };
    }
    let Some(seq) = echo_seq else {
        return EchoDecision::DropNoSeq;
    };
    if seq < last_local_seq {
        EchoDecision::DropStale
    } else {
        EchoDecision::Converge { seq }
    }
}

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
    /// Highest [`holon_api::write_seq::WriteSeq`] this editor has authored (via
    /// a content keystroke) or accepted (from a converged external write).
    /// The data-sync convergence guard drops any echo whose `write_seq` is
    /// strictly less than this — a stale/reordered CDC echo of an earlier
    /// keystroke — while still converging genuinely newer authority states
    /// (a `split_block` truncation issued after the last keystroke carries
    /// a greater seq). Starts at `WriteSeq::ZERO`: before the user types,
    /// every echo converges (correct seeding). See `holon_api::write_seq`
    /// for why content comparison cannot substitute (inline-mark stripping
    /// rewrites the stored value).
    last_local_seq: i64,
    /// Cancelled on drop. Subscribes to `MutableText.remote_deltas()`
    /// and splices remote edits into InputState via
    /// `replace_text_in_range_silent`.
    _remote_delta_subscription: Option<Task<()>>,
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
    /// Last window-focus state observed by the render-path reconcile gate
    /// (`focus_transition`). Used to detect the frame where focus first arrives
    /// (false→true) so the NO-CELL builder backstop can re-sync a stale
    /// `InputState` from the live backend content *once*, before the user has
    /// typed — the backstop for a no-cell editor's data-sync subscription being
    /// orphaned by a row-set rebuild (split/join/navigation replaces the
    /// per-row `Mutable` cell). Cell-attached editors bypass this gate
    /// (Increment G).
    prev_focused: std::cell::Cell<bool>,
    #[cfg(feature = "mobile")]
    /// The soft-keyboard focus generation this editor claimed on its last
    /// focus-gain (see `crate::mobile::editor_focus_gained`). Passed back on
    /// blur so a stale editor's late-arriving blur cannot hide the keyboard
    /// after a successor already claimed focus. Zero = never gained focus.
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
        // synchronous borrow, already used below to seed `previous_text`).
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
            InputState::new(window, cx)
                .auto_grow(1, usize::MAX)
                .default_value(&seed_value)
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
                        this.note_focus_gained_mobile();

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
                        let _ = (this, entity, cx);
                    }
                    InputEvent::Blur => {
                        #[cfg(feature = "mobile")]
                        this.note_focus_lost_mobile(cx);

                        let value = entity.read(cx).value().to_string();
                        let action = ctrl.lock().unwrap().on_blur(&value);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);
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
                        execute_action(action, &services_clone, this.input.entity_id(), cx);

                        // CRDT: compute local delta and apply through the
                        // view model. The Loro-backed cell filters our own
                        // writes via origin == "ui_local".
                        let vm = ctrl.lock().unwrap();
                        if vm.has_cell() {
                            let prev = this.previous_text.clone();
                            if text != prev {
                                if vm.current_text().as_deref() != Some(text.as_str()) {
                                    for op in compute_text_delta(&prev, &text) {
                                        if let Err(e) = vm.apply_local(op) {
                                            tracing::error!("apply_local failed: {}", e);
                                        }
                                    }
                                }
                                this.previous_text = text;
                            }
                        } else if text != this.previous_text
                            && !RowOrigin::from_id(&this.row_id).is_creation_placeholder()
                        {
                            // No Cell<String> attached (SqlOnly / no-Loro
                            // mode). The per-keystroke Loro pipeline is
                            // absent — fall back to `set_field("content")`
                            // so the typed text lands in the backend before
                            // the next transition. Without this, keystrokes
                            // only mutate the local InputState and are
                            // silently lost when the ReactiveRowSet rebuilds
                            // (e.g. on a later SplitBlock on another row).
                            //
                            // NEVER for a creation slot (`block:__virtual:<parent>`):
                            // it has no real block, so this `set_field` is a
                            // silent no-op write against a nonexistent id that
                            // ALSO poisons the undo stack with a virtual-id entry
                            // (BugFunnel dogfood #4). The slot's text is committed
                            // via `create` on Enter (`commit_creation_slot`); it
                            // must not be persisted per-keystroke.
                            // Stamp a monotonic ordering token on this content
                            // write and record it as our last local sequence.
                            // The provider persists it into `block_raw.write_seq`
                            // (same UPDATE as `content`), it echoes back through
                            // CDC, and the data-sync guard below drops any echo
                            // whose seq is older than this — the fix for the
                            // vault-scale "typing resets the block" reset. Must
                            // be set BEFORE dispatch so a fast echo can't race a
                            // not-yet-recorded seq.
                            let seq = holon_api::write_seq::next();
                            this.last_local_seq = seq.get();
                            let mut params = std::collections::HashMap::new();
                            params
                                .insert("id".into(), holon_api::Value::String(this.row_id.clone()));
                            params.insert(
                                "field".into(),
                                holon_api::Value::String("content".to_string()),
                            );
                            params.insert("value".into(), holon_api::Value::String(text.clone()));
                            params.insert("write_seq".into(), holon_api::Value::Integer(seq.get()));
                            services_clone.dispatch_intent(
                                holon_frontend::operations::OperationIntent::new(
                                    "block".into(),
                                    "set_field".into(),
                                    params,
                                ),
                            );
                            this.previous_text = text;
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
                    (content, echo_seq)
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
                while let Some((new_value, echo_seq)) = stream.next().await {
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
                                    let input = this.input.clone();
                                    let current = input.read(cx).value().to_string();
                                    match evaluate_data_sync_echo(
                                        &current,
                                        &new_value,
                                        echo_seq,
                                        this.last_local_seq,
                                    ) {
                                        EchoDecision::InSync { advance_to } => {
                                            // Echo of our own latest write (or a
                                            // redundant emit). Advance the
                                            // high-water mark so a later reordered
                                            // echo of an even earlier keystroke is
                                            // still seen as stale.
                                            if let Some(s) = advance_to {
                                                this.last_local_seq = this.last_local_seq.max(s);
                                            }
                                        }
                                        EchoDecision::DropNoSeq => {
                                            // Content changed but the row carries
                                            // no `write_seq` token — a schema /
                                            // projection regression. Fail LOUD and
                                            // DROP: converging blindly here is
                                            // exactly the stale-echo data loss we
                                            // are fixing.
                                            tracing::error!(
                                                target: "editor.data_sync",
                                                row_id = %this.row_id,
                                                current = %current,
                                                new = %new_value,
                                                "data-sync echo has no write_seq column; \
                                                 dropping (schema/projection regression)"
                                            );
                                        }
                                        EchoDecision::DropStale => {
                                            if caret_probe() {
                                                eprintln!(
                                                    "[data-sync] DROP stale echo seq={echo_seq:?} \
                                                     < last_local={} current={current:?} \
                                                     new={new_value:?}",
                                                    this.last_local_seq
                                                );
                                            }
                                        }
                                        EchoDecision::Converge { seq } => {
                                            if caret_probe() {
                                                eprintln!(
                                                    "[data-sync] apply seq={seq} last_local={} \
                                                     current={current:?} new={new_value:?}",
                                                    this.last_local_seq
                                                );
                                            }
                                            // Adopt the authority's seq as our new
                                            // high-water mark and converge. Keeps
                                            // `previous_text` in lockstep so the
                                            // re-entrant Change writes nothing back.
                                            this.last_local_seq = seq;
                                            this.converge_input(
                                                "data_sync",
                                                &new_value,
                                                window,
                                                cx,
                                            );
                                        }
                                    }
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
                                    let (ime_active, current, focused) = {
                                        let state = input.read(cx);
                                        (
                                            state.ime_marked_range().is_some(),
                                            state.value().to_string(),
                                            state.focus_handle(cx).is_focused(window),
                                        )
                                    };
                                    // IME guard: never converge mid-composition.
                                    if ime_active {
                                        return;
                                    }
                                    // Focus/idle gate (mirrors the data path):
                                    // a focused, actively-typing editor keeps
                                    // its in-flight text; a focused-but-idle
                                    // editor — e.g. the just-focused merge
                                    // target after a join — DOES converge, and
                                    // that is how it receives the merged
                                    // content. `user_idle` := no unflushed
                                    // keystroke (previous_text == InputState).
                                    let user_idle = this.previous_text == current;
                                    if focused && !user_idle {
                                        return;
                                    }
                                    this.converge_input(
                                        "remote_delta",
                                        &cell.current(),
                                        window,
                                        cx,
                                    );
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
            grab_focus_and_seed_caret(&input, window, cx, services.as_ref(), &row_uri, true);
        }

        Self {
            input,
            controller,
            row_id,
            services,
            nav,
            bounds_registry,
            popup_scroll: ScrollHandle::new(),
            _data_subscription,
            _focus_subscription,
            previous_text,
            _remote_delta_subscription,
            last_local_seq: holon_api::write_seq::WriteSeq::ZERO.get(),
            prev_focused: std::cell::Cell::new(false),
            #[cfg(feature = "mobile")]
            focus_gen: std::cell::Cell::new(0),
        }
    }

    /// Mobile soft-keyboard focus hooks, keeping `focus_gen` in lockstep with
    /// the generation claimed on gain so blur can prove it is not stale.
    /// No-ops off `feature = "mobile"`.
    #[cfg(feature = "mobile")]
    pub fn note_focus_gained_mobile(&self) {
        self.focus_gen.set(crate::mobile::editor_focus_gained());
    }

    #[cfg(feature = "mobile")]
    pub fn note_focus_lost_mobile(&self, cx: &mut App) {
        crate::mobile::editor_focus_lost(cx, self.focus_gen.get());
    }

    /// The soft-keyboard focus generation this editor last claimed (0 if never
    /// focused). Callers that hold a live `entity.read(cx)` borrow — which
    /// blocks the `&mut cx` that `editor_focus_lost` needs — read this and pass
    /// it to `crate::mobile::editor_focus_lost` directly.
    #[cfg(feature = "mobile")]
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
    /// Sets `previous_text` in lockstep so the re-entrant `InputEvent::Change`
    /// (gpui_component's "silent" splice is NOT silent — it emits Change
    /// unconditionally) computes an empty delta and writes nothing back to the
    /// authority. This is the "write only genuine user edits" invariant.
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
        let target = {
            let vm = self.controller.lock().unwrap();
            vm.current_text().unwrap_or_else(|| sql_default.to_string())
        };
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
        self.controller.lock().unwrap().rebaseline(&target);
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
        input.update(cx, |state, cx| {
            let byte_offset = match restored {
                Some(new_codepoint) => state.text().char_index_to_offset(new_codepoint),
                None => preserved_caret(prior_cursor, &target),
            };
            let pos = state.text().offset_to_position(byte_offset);
            state.set_cursor_position(pos, window, cx);
        });
        // Lockstep: the deferred re-entrant Change now sees text ==
        // previous_text → empty delta → no spurious write-back.
        self.previous_text = target;
    }
}

/// Clamp a caret byte offset captured before an absolute `set_value` onto the
/// new text: cap at its length and snap down to the nearest UTF-8 char boundary
/// so the restored caret is always a valid offset. The click position survives
/// convergence in SqlOnly mode (no Loro anchor); when the new text is shorter
/// the caret pins to the end.
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
            let max_h = popup_max_height_px(window.viewport_size().height.into());
            ctrl.popup_state()
                .map(|s| render_popup(&s, &self.bounds_registry, &self.popup_scroll, max_h, cx))
        };

        let window_handle = window.window_handle();
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
                let editor_entity = editor_entity.clone();
                move |_: &Enter, window, cx: &mut App| {
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
                            // The creation slot (`block:__virtual:<parent>`) has
                            // no real block yet — Enter must ONLY commit/create
                            // (`ViewEventHandler::handle_text_sync`'s
                            // CreationPlaceholder arm), never dispatch a
                            // structural op against the virtual id.
                            // `structural_block_action`'s split_block would
                            // target a block that (from the CRDT/SQL side) never
                            // existed under this id — `dispatch_intent_chain`
                            // would 404 it as "Block not found" even though the
                            // commit itself succeeded. This mirrors the headless
                            // PBT driver's `commit_creation_slot`, which never
                            // chains a structural op either. Gate on the
                            // editor's OWN row (the `ctrl` doing the commit),
                            // not `target_id` — a momentarily stale
                            // `focused_block` must not route a slot Enter into
                            // the structural path.
                            if RowOrigin::from_id(&row_id).is_creation_placeholder() {
                                let live_text = input.read(cx).value().to_string();
                                if let Some(commit) =
                                    ctrl.lock().unwrap().commit_creation_slot(&live_text)
                                {
                                    services.dispatch_intent(commit);
                                    // The render backstop (`converge_on_render`)
                                    // only reconciles a no-cell editor's
                                    // `InputState` on a focus edge — but focus
                                    // stays on this same slot across the commit,
                                    // so nothing would otherwise clear the
                                    // committed text before the "type here to
                                    // add a new block" placeholder repaints.
                                    // Force convergence now (idempotent, same
                                    // path the render backstop uses) instead of
                                    // waiting for a blur/refocus that may never
                                    // come.
                                    let editor_entity = editor_entity.clone();
                                    cx.spawn(async move |cx| {
                                        let _ = cx.update_window(window_handle, |_, window, cx| {
                                            editor_entity.update(cx, |this, cx| {
                                                this.converge_input(
                                                    "post_commit_clear",
                                                    "",
                                                    window,
                                                    cx,
                                                );
                                            });
                                        });
                                    })
                                    .detach();
                                }
                                cx.stop_propagation();
                                return;
                            }
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
                    // Creation slot: no real block to join — swallow the
                    // structural gesture (`structural_block_action` asserts
                    // against placeholder ids).
                    if RowOrigin::from_id(&row_id).is_creation_placeholder() {
                        cx.stop_propagation();
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
                    // Creation slot: nothing to indent — swallow (see Backspace).
                    if RowOrigin::from_id(&row_id).is_creation_placeholder() {
                        cx.stop_propagation();
                        return;
                    }
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
                    // Creation slot: nothing to outdent — swallow (see Backspace).
                    if RowOrigin::from_id(&row_id).is_creation_placeholder() {
                        cx.stop_propagation();
                        return;
                    }
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

fn render_popup(
    state: &PopupState,
    bounds_registry: &BoundsRegistry,
    scroll: &ScrollHandle,
    max_height_px: f32,
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

    // Keep the keyboard-selected entry inside the scroll viewport as the user
    // arrows through a list longer than `max_height_px`.
    scroll.scroll_to_item(state.selected_index);

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
mod echo_suppression {
    //! Directed regression tests for the op-versioned data-sync echo guard —
    //! the fix for the vault-scale P1 "typing `[[` (or any edit) resets the
    //! whole block to its pre-typing content".
    //!
    //! The failure is a focused editor converging to a STALE/reordered CDC echo
    //! of an earlier keystroke. These exercise the pure decision function
    //! [`evaluate_data_sync_echo`] the data-sync closure delegates to,
    //! modelling an INJECTED-DELAY, in-flight-typing timeline
    //! deterministically (no gpui window, no real latency needed).
    //!
    //! RED-FIRST equivalence: the old policy converged whenever the editor was
    //! "idle" (`prev_synced == current`), which is true the instant a keystroke
    //! settles — so `stale_echo_while_typing_ahead_is_dropped` below would have
    //! CONVERGED (reset the block) under the old code. The seq guard makes it a
    //! drop.

    use super::EchoDecision;
    use super::evaluate_data_sync_echo;

    // A block seeded at boot carries write_seq 0 (the column default) until the
    // editor writes it. The editor's own keystrokes carry strictly-increasing
    // process-global sequences (holon_api::write_seq::next()).
    const SEED: i64 = 0;

    #[test]
    fn stale_echo_while_typing_ahead_is_dropped() {
        // Timeline: user typed "ab" (seq 10) then "abc" (seq 11); InputState is
        // now "abc" and last_local_seq is 11. The CDC echo of the EARLIER "ab"
        // write (seq 10) arrives late. It must be DROPPED — converging would
        // reset the visible text backwards to "ab". This is the exact P1.
        let d = evaluate_data_sync_echo("abc", "ab", Some(10), 11);
        assert_eq!(d, EchoDecision::DropStale);
    }

    #[test]
    fn pre_typing_stale_echo_is_dropped() {
        // The reported symptom: block content is "Block 07-010 ..." pre-typing.
        // The user types, advancing last_local_seq to 600. A lagged echo of the
        // pre-typing content (an older, smaller seq) must not resurrect it.
        let d = evaluate_data_sync_echo(
            "Block 07-010 ...hello",
            "Block 07-010 ...", // pre-typing content
            Some(305),
            600,
        );
        assert_eq!(d, EchoDecision::DropStale);
    }

    #[test]
    fn split_truncation_after_last_keystroke_still_converges() {
        // A split_block issued AFTER the last keystroke gets a greater seq, so
        // the surviving (reused) editor still converges to the truncated content
        // while it owns focus — the property the old idle-heuristic preserved and
        // the seq guard must keep.
        let d = evaluate_data_sync_echo("hello world", "hello", Some(12), 11);
        assert_eq!(d, EchoDecision::Converge { seq: 12 });
    }

    #[test]
    fn equal_seq_external_write_converges() {
        // Non-editor writers (split/join/org) don't bump write_seq, so the row
        // retains the editor's last seq; their echo carries seq == last_local and
        // a DIFFERENT value → converge (they changed content, not the token).
        let d = evaluate_data_sync_echo("hello world", "hello", Some(11), 11);
        assert_eq!(d, EchoDecision::Converge { seq: 11 });
    }

    #[test]
    fn self_echo_is_in_sync_and_advances_high_water() {
        // The confirming echo of our own latest write equals current InputState.
        let d = evaluate_data_sync_echo("abc", "abc", Some(11), 11);
        assert_eq!(
            d,
            EchoDecision::InSync {
                advance_to: Some(11)
            }
        );
    }

    #[test]
    fn pre_typing_editor_converges_to_external_seed() {
        // Before the user types (last_local_seq == SEED == 0) every external
        // state is at least as new → converge. This is correct seeding: a
        // freshly focused editor adopts the authority content.
        let d = evaluate_data_sync_echo("stale", "fresh from peer", Some(1), SEED);
        assert_eq!(d, EchoDecision::Converge { seq: 1 });
    }

    #[test]
    fn missing_seq_on_changed_content_fails_loud_and_drops() {
        // A content change with no write_seq token is a schema/projection
        // regression: drop (never converge blindly) — the loud tracing::error!
        // lives at the call site.
        let d = evaluate_data_sync_echo("abc", "different", None, 11);
        assert_eq!(d, EchoDecision::DropNoSeq);
    }

    #[test]
    fn missing_seq_but_in_sync_is_noop_without_advance() {
        // No token, but the echo equals current — a benign redundant emit. In
        // sync, and there is no seq to advance the high-water mark to.
        let d = evaluate_data_sync_echo("abc", "abc", None, 11);
        assert_eq!(d, EchoDecision::InSync { advance_to: None });
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
