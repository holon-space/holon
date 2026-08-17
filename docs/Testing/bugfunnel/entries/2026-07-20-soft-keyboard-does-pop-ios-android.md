---
id: 2026-07-20-soft-keyboard-does-pop-ios-android
date: 2026-07-20
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Soft keyboard does not pop up on iOS + Android release builds when a text
  input gains an active cursor — both editable blocks AND the new quick-open
  search box (caret renders, no keyboard). ROOT CAUSE (search box, confirmed
  by code archaeology): the search modal (`frontends/gpui/src/search_ui.rs`,
  added 2026-07-19 in commit `1a2e0da`) focuses its
  `gpui_component::InputState` via `input.focus()` but NEVER joined the mobile
  keyboard-generation protocol (`crate::mobile::editor_focus_gained/lost` →
  `gpui_mobile::show_keyboard`), which is the ONLY thing that raises the
  platform IME (the fork does not auto-raise on gpui focus). So the search box
  showed a caret but no keyboard. Worse, focusing the search input BLURS the
  active editor, whose `editor_focus_lost` then ran its ~150ms deferred hide
  (search never advanced `KEYBOARD_FOCUS_GENERATION`), so opening search over
  a focused block ALSO dropped the editor's keyboard — making the editor look
  regressed. Verified NOT a keyboard-code regression: the editor
  `editor_focus_gained/lost` call sites are byte-unchanged since the
  2026-07-17 "flaky-keyboard fix" (`e5628548`); the gpui-mobile
  (`68df9dd→75410fc`) and gpui/zed (`e4cf5b15→ef2f1164`) pin bumps on
  2026-07-19 (commit `4807c280`) were screenshot/render-only, no focus/IME
  change; fork keyboard code (becomeFirstResponder/InputConnection) unchanged
  since before the 2026-07-09 known-good note.
source_line: 801
---

## Bug

Soft keyboard does not pop up on iOS + Android release builds when a text
input gains an active cursor — both editable blocks AND the new quick-open
search box (caret renders, no keyboard). ROOT CAUSE (search box, confirmed
by code archaeology): the search modal (`frontends/gpui/src/search_ui.rs`,
added 2026-07-19 in commit `1a2e0da`) focuses its
`gpui_component::InputState` via `input.focus()` but NEVER joined the mobile
keyboard-generation protocol (`crate::mobile::editor_focus_gained/lost` →
`gpui_mobile::show_keyboard`), which is the ONLY thing that raises the
platform IME (the fork does not auto-raise on gpui focus). So the search box
showed a caret but no keyboard. Worse, focusing the search input BLURS the
active editor, whose `editor_focus_lost` then ran its ~150ms deferred hide
(search never advanced `KEYBOARD_FOCUS_GENERATION`), so opening search over
a focused block ALSO dropped the editor's keyboard — making the editor look
regressed. Verified NOT a keyboard-code regression: the editor
`editor_focus_gained/lost` call sites are byte-unchanged since the
2026-07-17 "flaky-keyboard fix" (`e5628548`); the gpui-mobile
(`68df9dd→75410fc`) and gpui/zed (`e4cf5b15→ef2f1164`) pin bumps on
2026-07-19 (commit `4807c280`) were screenshot/render-only, no focus/IME
change; fork keyboard code (becomeFirstResponder/InputConnection) unchanged
since before the 2026-07-09 known-good note.

## Missing piece

The platform soft-keyboard raise is invisible to the headless keystone AND
to windowed GPUI tests (no IME in CI) — no rung observes whether
`show_keyboard` fired on focus-gain. Secondary COVERAGE: no test asserts a
newly-focused text input (search box OR editor) invokes
`editor_focus_gained` / claims a keyboard generation; the rung that WOULD
catch it is a gpui-windowed assertion that opening search (and focusing a
block) calls the mobile focus hook, plus a live device/simulator IME-visible
check.

## Remedy

FIXED 2026-07-20 (in-repo, `frontends/gpui/src/search_ui.rs`):
`SearchUiState` now carries `focus_gen` and joins the SAME protocol —
`open()` claims a generation + raises the keyboard (this ALSO cancels the
just-blurred editor's deferred hide, so the keyboard stays up across
editor→search), `close(cx)` releases it (generation-guarded hide). Gates:
`cargo check -p holon-gpui` clean (desktop) + `--no-default-features
--features mobile` clean. Editor-only tap→keyboard path unchanged and could
NOT be reproduced as a code defect here (two triggers wired, code
unchanged); needs on-device confirmation via the existing
`tracing::debug!("soft keyboard: show (editor focus)")` to distinguish
not-called vs platform-noop.
