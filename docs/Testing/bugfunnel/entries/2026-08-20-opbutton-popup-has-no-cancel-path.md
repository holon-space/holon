---
id: 2026-08-20-opbutton-popup-has-no-cancel-path
date: 2026-08-20
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The op_button param-collection popup had no cancel path — Escape was unbound,
  outside-click was ignored, and menus stacked across rows; the only exits were
  dispatching a value or closing the whole Settings modal.
---

## Bug

Found by dogfooding the popup-driven op_button set_field (the feature this lane
adds). Once the param-collection popup was open there was no way to back out of
it without committing:

- **Escape** closed neither the popup nor (while it was open) the Settings modal.
- **Outside-click** was ignored — clicking elsewhere did not dismiss the popup.
- Popups **stacked across rows**: opening gcal's then todoist's left two open at
  once.

The only exits were dispatching a value or closing the entire Settings modal.

## Root cause

The op_button popup (`frontends/gpui/src/render/builders/op_button.rs`) simply
wired no dismissal affordances: no outside-click listener, and no focused element
to receive Escape (the op_button is not focused, and gpui dispatches keys only
along root→focused, so an unfocused wrapper never saw the keystroke). Stacking
followed directly — with nothing closing popup A, opening popup B left both.

A first analysis claimed these behaviors were impossible under the per-row cached
-view design and needed promotion to an app-level popup surface; a frontier
pressure-test against the pinned gpui source REFUTED that. All three are provided
by in-tree precedent without leaving the button-hosted design.

## Missing piece

No test drove the cancel/second-open interactions. The windowed rung
(`frontends/gpui/tests/settings_integrations_setfield_popup_windowed.rs`) opened
the popup and completed it, but never pressed Escape, never clicked outside, and
never opened a second row's popup — so the absent cancel path was never generated
(COVERAGE).

## Remedy

**FIXED**, staying on the button-hosted (Option A) design:

- **Outside-click:** `.on_mouse_down_out` on the WRAPPER div (trigger + menu) —
  the Settings modal's own dismissal mechanism (`frontends/gpui/src/lib.rs:728`),
  a capture-phase window-wide listener; no backdrop, no z-order. Wrapper (not
  menu-only) so an own-trigger click does not close-then-reopen.
- **Escape:** a `FocusHandle` created and focused when the popup opens, tracked on
  the wrapper (`.track_focus`), with `.on_key_down` matching "escape" — the
  pattern from `frontends/gpui/src/search_ui.rs:409`.
- **Single-open:** emergent from outside-click — opening another row's op_button
  is a click outside the first, which `on_mouse_down_out` dismisses. No shared
  state.

Neither cancel path dispatches an operation (closing is not a mutation).

MECHANISM ATTRIBUTION (delta-verify caught this, and it matters for prod): the
`opening_a_second_rows_op_button_closes_the_first` rung is NOT teeth for
`on_mouse_down_out` — in the windowed environment, opening the second popup is a
STRUCTURAL REBUILD, and the ephemeral-cache wipe (`entity_view_registry.rs`,
`wipe_ephemeral`) drops the first popup's non-state-bearing entity
on its own; that rung stays green even with `on_mouse_down_out` guarded
`if false`. In the LIVE app the pre-fix dogfood observed popups STACKING — i.e.
that rebuild/wipe does not fire there — so `on_mouse_down_out` is the
load-bearing single-open (and outside-click) mechanism in production. The rung
that actually pins the handler is `outside_click_on_inert_space_closes_the_
popup_without_dispatching`: it clicks inert row text (no op_button, no toggle →
no dispatch and no structural rebuild → the wipe cannot fire), so only the
handler can close the popup. Teeth-proven: with `on_mouse_down_out` guarded
`if false` it goes RED ("must close it via on_mouse_down_out"); restored
(sha256-verified) it goes GREEN.

Red-for-the-right-reason, then green (windowed file):

```
RED:  Escape must close the param-collection popup (left 1, right 0)
      outside-click on inert space must close it via on_mouse_down_out (if-false teeth)
GREEN: outside_click_on_inert_space_closes_the_popup_without_dispatching ... ok
       escape_closes_the_param_popup_without_dispatching ... ok
       opening_a_second_rows_op_button_closes_the_first ... ok
```

Known minor deviation, tracked separately: in the headless windowed harness,
clicking a second row's button closes the first but does not open the second on
that same click (both end closed; the required "never two stacked" invariant
holds). Likely a headless synchronous-refresh artifact of the capture-phase
dismiss; the dogfood re-run confirms the real-app behavior.
