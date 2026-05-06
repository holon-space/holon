# `Blur` was a fake PBT transition — removed (and PressKey-Escape with it)

**Date**: 2026-05-08
**Continues / corrects**: `devlog/2026-05-08-124516-blur-escape-active-editor-stale-after-nav.md`
**Status**: LANDED

## What was wrong with the prior fix

The previous devlog framed `Blur`'s panic as a ref-state staleness bug and "fixed" it by clearing `state.active_editor` on every nav transition. The user pushed back: tests should only do what users do — click and type. The deeper bug is that **`Blur` itself isn't a user action** — pressing Escape in the editor has no production effect.

Evidence: `EditorViewModel::on_key(EditorKey::Escape)` at `crates/holon-frontend/src/editor_view_model.rs:255` returns `EditorAction::Propagate` when no popup is open. The editor's GPUI capture handler at `frontends/gpui/src/views/editor_view.rs:586-592` only `stop_propagation()`s when the action is non-Propagate/non-None, so Escape walks past the editor's bindings and finds no upstream handler — `dispatch_keystroke` returns false. Real users blur by clicking elsewhere; platform focus shift handles the rest.

The PBT was modeling a "user action" that the production code intentionally drops on the floor.

## Changes

- **Deleted** `crates/holon-integration-tests/src/pbt/transitions/blur.rs` (the transition file).
- **Removed** `Blur` from `transitions/mod.rs` (mod decl, `pub use`, enum variant) and from `SutHandle::apply_blur` in `transition_dispatch.rs` and `sut.rs::apply_blur`.
- **Removed** `Key::Escape` from `PressKey::weighted_generator`'s `chord_strategy` in `transitions/press_key.rs` — same reason: production drops Escape, the test driver requires `handled=true`, panic is inevitable.
- **Updated** doc comments in `reference_state.rs`, `ui_harness.rs`, `edit_via_view_model.rs`, `sut.rs` that listed `Blur` as one of the atomic editor primitives.
- **Drive-by**: bare `_` for unused trait-required `&ReferenceState` params in five touched files (archlint `no-underscore-params`).

## What's kept (and why)

The four nav transitions (`NavigateFocus`, `NavigateHome`, `NavigateBack`, `NavigateForward`) still call `commit_active_editor_if_changed()` + `state.active_editor = None;` in their `apply_to_ref`. This is **not** a fake user action — it's the ref-state correctly modeling a side-effect of the user's click:

- User clicks a sidebar entry (real user action).
- GPUI moves platform focus to the sidebar selectable.
- The editor's `InputState` emits `InputEvent::Blur`.
- `EditorViewModel::on_blur` dispatches `set_field` if text changed.

Without modeling this side-effect, subsequent `TypeChars` / `MoveCursor` / `DeleteBackward` / `PressKey` transitions would fire when the SUT no longer has an editor focused — and the same "GPUI keystroke not consumed" panic would resurface, just in a different transition. The active_editor cleanup represents what the production system does, not what the user does.

## Verification

50-step `gpui_ui_pbt` on 3 seeds (`PROPTEST_SEED=1`, `2`, `3`):

```
=== Seed 1 ===  passed 48/50, no panic
=== Seed 2 ===  passed 44/50, no panic
=== Seed 3 ===  passed 47/50, no panic
```

Edit-path transitions (`FocusEditableText`, `TypeChars`, …) didn't fire in these particular seeds (random luck, same as the prior verification run), but the workspace still compiles, the file-per-transition arch tests still pass, and no transition-skip ratio changed.

## Files

- Deleted: `crates/holon-integration-tests/src/pbt/transitions/blur.rs`
- `crates/holon-integration-tests/src/pbt/transitions/mod.rs` — removed mod, pub use, enum variant
- `crates/holon-integration-tests/src/pbt/transition_dispatch.rs` — removed `apply_blur` from `SutHandle`
- `crates/holon-integration-tests/src/pbt/sut.rs` — removed `apply_blur` impl, updated comment
- `crates/holon-integration-tests/src/pbt/transitions/press_key.rs` — dropped Escape from chord_strategy
- `crates/holon-integration-tests/src/pbt/transitions/focus_editable_text.rs` — removed `Some("Blur")` arm in last-transition heuristic
- `crates/holon-integration-tests/src/pbt/transitions/edit_via_view_model.rs` — comment update
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — doc updates on `active_editor` and `atomic_editor_enabled`
- `crates/holon-integration-tests/src/pbt/ui_harness.rs` — doc update
- `crates/holon-integration-tests/src/pbt/transitions/navigate_focus.rs` — comment now describes the production focus-shift cause, not "panics Blur"

## Lesson

Before adding a "fix" that mutates ref-state to make a panic go away, ask: **does this transition correspond to something a user can do that production actually responds to?** If production drops the action silently, the test is asserting on a non-feature and the fix is to delete the transition, not to compensate in the model.
