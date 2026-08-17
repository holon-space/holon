---
id: 2026-08-10-fixing-fixed-cmd-enter-mouse-click
date: 2026-08-10
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Fixing `cycle_task_state` fixed Cmd+Enter but NOT the mouse click: the
  `state_toggle` click never dispatches `cycle_task_state` at all, so it still
  walks the hardcoded widget ring and reproduces the same out-of-vocabulary
  write.
source_line: 1198
---

## Bug

(found while fixing task #79 — the ENGINE cycle path; the click affordance
was then checked and diverges) **Fixing `cycle_task_state` fixed Cmd+Enter
but NOT the mouse click: the `state_toggle` click never dispatches
`cycle_task_state` at all, so it still walks the hardcoded widget ring and
reproduces the same out-of-vocabulary write.**
`ReactiveEngineDriver::cycle_state_toggle` and the GPUI `on_mouse_down`
resolve `focus_path::state_toggle_cycle_intent`
(`crates/holon-frontend/src/focus_path.rs:352`), which computes the next
value from `render_eval::resolve_states`
(`crates/holon-api/src/render_eval.rs:133-152`, the third hardcoded ring)
and dispatches a plain `set_field` — bypassing the engine authority
entirely. Two affordances for one user-visible verb now disagree: the
keyboard is vocabulary-aware, the mouse is not, with identical data loss on
re-ingest (the task vanishes into body text). This is an ARCHITECTURE FORK,
deliberately not resolved in-lane: either the widget delegates to
`cycle_task_state` (engine stays the sole ring authority, deleting a ring —
but it changes the op the affordance fires, touching
GPUI/dioxus/`focus_path`/`sim_windowed_replay` and several op-name
assertions), or the vocabulary is threaded into the widget's `states` render
arg (smaller blast radius, keeps three rings alive).

## Missing piece

no draw clicks a `state_toggle` inside a document declaring `#+TODO:` — the
same missing generator arm as the F3 row, now shown to matter on a SECOND
affordance; secondary ORACLE, since nothing asserts that the keyboard and
mouse paths for one verb produce the same write

## Remedy

OPEN — architecture ruling needed (delegate-to-op vs thread-into-widget).
Recommendation: delegate, because it deletes a ring rather than feeding one,
leaving the engine as sole authority. Scope was deliberately not expanded
mid-lane.
