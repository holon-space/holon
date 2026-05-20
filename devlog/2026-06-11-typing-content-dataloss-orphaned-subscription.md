# Typing-content data-loss: orphaned per-row data-sync subscription

Date: 2026-06-11. Worktree `typing-content-dataloss` (jj base `yksvlrnk` =
main + a one-line duplicate-import repair to `event_infra_module.rs` that
unblocked worktree creation — main was committed-broken).

## Symptom

Task #4 from the previous handoff: a real data-loss bug, minimized to 11
deterministic transitions
(`noloro7_typing_content_divergence.captured.json.min.json`). An external
`ApplyMutation Update` sets a split-created block's content to
`"fC Lf6HZ3 Nb"`; a later UI `SplitBlock` at position 9 ends with that
block `""` and the new sibling holding the whole string — the split
dispatched at cursor 0 on **empty** editor text.

## Root cause (proven via layered render-path probes)

The engine was always correct (the split read `block_raw` and split
`"fC Lf6HZ3 Nb"` at the editor-reported cursor). The bug is purely on the
GPUI side: the **clicked editor's `InputState` was stale `""`** at split
time, so its reported cursor clamped to 0.

Why stale: the `EditorView`'s data-sync subscription
(`editor_view.rs`, `_data_subscription`) is bound to the per-row `Mutable`
cell that existed when the editor was first **cached** (`ctx.local`
entity cache, keyed `editable-text-{row}-{field}`). Every structural change
(split/join) **and** navigation rebuilds the `ReactiveRowSet` with a fresh
set of per-row cells — instrumentation showed two separate
`Created->INSERT(new cell)` events for the same block id, with no
`retain_keys`/`Deleted` between them, i.e. a second row-set. The cached
editor survives the rebuild but keeps subscribing to the **orphaned** old
cell. The external write lands on the **new** cell (`rendered_text` /
`editable_text` both read the live value fine), but the editor's
subscription — bound to the dead cell — emits exactly once (`""`) and never
again. Confirmed: a per-emission probe logged a single `EMIT ""` for the
whole run.

The design in `reactive_view_model.rs` explicitly *assumes* stable per-row
cell identity ("row-level updates flow through the per-row signal
automatically"); structural/navigation rebuilds violate that assumption for
editors that outlive the row-set in the entity cache.

## Fix

Render-path reconciliation in `render/builders/editable_text.rs`. The shell
re-renders this builder on the **new** cell's data signal, so `content`
(live `node.prop_str("content")`, == `node.data`) is always current here.
When the editor's `InputState` diverges from `content` **and the user
cannot be mid-typing**, push the live content into `InputState`:

```rust
if current != content && (!is_focused || just_focused) {
    input.update(cx, |state, cx| state.set_value(&content, window, cx));
}
```

`just_focused` is the false→true window-focus edge, tracked by a new
`EditorView::prev_focused: Cell<bool>` + `focus_arrived()`. Click-to-edit
focuses the editor *synchronously*, so `!is_focused` alone never fires at
the split — but the focus-arrival frame (user clicked, hasn't typed) is a
safe moment to re-sync from the backend before any keystroke. A
continuously-focused editor is left alone so in-flight typing is never
yanked.

The reconcile is **write-safe by construction**: it only ever sets
`InputState` to the live backend content, so any `Change`-triggered write is
idempotent (writes the same value back). It cannot introduce new content.

## Validation

- Min capture replay (`gpui_capture_replay` + `HOLON_CAPTURE`): **EXIT=0**
  (was deterministically red); SQL shows `UPDATE block_raw SET content =
  'fC Lf6HZ3'` — correct 9-char truncation.
- Headless `general_e2e_pbt_sql_only`: PASS (58s).
- `gpui_gherkin_replay` (deterministic windowed, committed fixture): PASS.
- Windowed `gpui_ui_pbt_no_loro` random sweeps: surfaced only **pre-existing
  engine-layer** faces — `#ir` (DeleteBackward) and `#ir`/`#+ir`
  (SplitBlock), both `trouble begins at: TursoProjection`. In the `#+ir`
  case `rendered_text` faithfully shows the engine's wrong `"#+ir"` — i.e.
  the widget correctly reflects the (wrong) engine value, so the reconcile
  is working; the engine content itself is wrong via the org-format/typing
  path (see blur_commit_ref_settle_fix_2026-06-11). Not regressions.

## Open / follow-up

- The focused-**idle** external-update path on an orphaned subscription is
  still a (rarer) gap — `just_focused` only covers the arrival frame. The
  principled fix is to stop orphaning (preserve per-row cell identity across
  structural/navigation rebuilds, or re-bind the editor's subscription when
  `node.data` identity changes). This render-path reconcile is the robust
  backstop; the deeper fix is the "Field authority and intent capture"
  Phase B/C work (UI.md).
- The `#ir`/`#+ir` engine faces remain open (pre-existing, out of scope).
