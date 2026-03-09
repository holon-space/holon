# PBT `Blur` panic: ref-state `active_editor` stale across NavigateFocus/Home/Back/Forward

**Date**: 2026-05-08
**Continues**: `devlog/2026-05-08-115502-bulk-external-add-empty-doc-weight.md`
**Status**: FIXED

## Symptom

Once `BulkExternalAdd` weighting unblocked edit-path transitions, `gpui_ui_pbt` started panicking at `crates/holon-integration-tests/src/pbt/sut.rs:2345`:

```
Blur: escape failed: GPUI keystroke not consumed: keystroke="escape" modifiers=[]
```

Concrete trace at the panic seed:

```
Step 21: FocusEditableText  → ref_state.active_editor = Some(...)
Step 22: PinBlock           → no effect on focus
Step 23: NavigateFocus      → moves Main focus, clears focused_entity_id, leaves active_editor SET
Step 24: Blur               → ref precondition passes (active_editor.is_some()),
                              SUT sends Escape, GPUI focus chain has no Input context,
                              dispatch_keystroke returns false, send_raw_keystroke bails.
```

## Root cause

The reference model's `active_editor` survives across navigation transitions, but the SUT's GPUI focus does not. When NavigateFocus moves the focus to a new block, GPUI's editor loses platform focus and emits `InputEvent::Blur`. After that point, Escape no longer matches the `Input` context's `KeyBinding::new("escape", Escape, Some(CONTEXT))` (gpui-component `input/state.rs:129`), so `dispatch_keystroke` returns `false` and `send_raw_keystroke` reports `handled=false`.

The four navigation transitions all clear `focused_entity_id` and `focused_cursor` for their region, but none of them clear `state.active_editor`:

- `crates/holon-integration-tests/src/pbt/transitions/navigate_focus.rs::apply_to_ref`
- `crates/holon-integration-tests/src/pbt/transitions/navigate_home.rs::apply_to_ref`
- `crates/holon-integration-tests/src/pbt/transitions/navigate_back.rs::apply_to_ref`
- `crates/holon-integration-tests/src/pbt/transitions/navigate_forward.rs::apply_to_ref`

In production, navigation triggers GPUI focus loss → editor emits `InputEvent::Blur` → `EditorViewModel::on_blur` returns `Execute(set_field)` if text changed (commits the in-memory edit). The ref model needs to mirror that pair: commit pending text, clear `active_editor`.

## Fix

Each of the four `apply_to_ref` implementations now ends with:

```rust
state.commit_active_editor_if_changed();
state.active_editor = None;
```

`commit_active_editor_if_changed()` (already in `reference_state.rs:659`) flushes `editor.in_memory_content` into the underlying block before zeroing the cell.

Drive-by: `apply_to_sut(&self, _state: ...)` → `apply_to_sut(&self, _: ...)` in all four (archlint `no-underscore-params` was triggered by the surrounding edit; bare `_` is the trait-required form).

## Verification

50-step run on 3 random seeds:

```
=== Seed 1 ===  passed 48/50, no panic
=== Seed 2 ===  passed 44/50, no panic
=== Seed 3 ===  passed 47/50, no panic
```

Pre-fix (same `BulkExternalAdd` weighting active): panicked at step 24/50 on the seed that landed `FocusEditableText → … → NavigateFocus → Blur`.

## Notes / open follow-ups

- Edit-path transitions (`FocusEditableText`, `TypeChars`, `SplitBlock`, `Blur`, …) didn't fire in any of the 3 verification seeds — random luck. Coverage is uneven. If we want forced exposure of the edit path, the `HOLON_PBT_WEIGHTS=FocusEdit*:50,Type*:50,Split*:50` env var is the lever.
- `PressKey` already had identical commit-and-clear logic (`press_key.rs:129` for `Enter`/structural chords; `:169` for `Escape`-as-blur). The four nav transitions were the leftover gap.
- Same gap may exist for any future transition that causes a focus shift away from the active editor (e.g. closing a popup, switching view-modes via `SwitchView`). Worth a cross-reference scan if related panics resurface.

## Files

- `crates/holon-integration-tests/src/pbt/transitions/navigate_focus.rs` — added commit + clear, removed `_state` underscore param
- `crates/holon-integration-tests/src/pbt/transitions/navigate_home.rs` — same
- `crates/holon-integration-tests/src/pbt/transitions/navigate_back.rs` — same
- `crates/holon-integration-tests/src/pbt/transitions/navigate_forward.rs` — same

## Logs

- `/tmp/gpui_ui_pbt_v3.log` — first post-fix run (45/50 pass, no panic)
- `/tmp/gpui_pbt_seed_{1,2,3}.log` — three-seed verification
