---
id: 2026-09-18-focus-editable-text-view-local-enter
date: 2026-09-18
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  `FocusEditableText` reds against the TUI click's emission barrier only under
  machine load; the mechanism behind the red is unproven and the failure is
  not fixed.
---

## Bug

`frontends/tui` `tui_ui_pbt` panics when the drawn sequence reaches
`FocusEditableText` on a main-panel text row:

```
[SutFocusWrite::apply_focus_editable_text] click_entity(main, block:c1) failed:
await_chord_settled: engine did not quiesce: emissions did not quiesce after
dispatch — CDC pipeline stuck?: emissions did not advance past tick 1 within 2s
  — crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2903
```

Load-correlated. With `HOLON_PBT_WEIGHTS=FocusEditableText:2000`, the same
unmodified code is:

- **3 of 16 red** at load 234-264 (`lane-logs/featA-r6.log`,
  `lane-logs/featB-r11.log`, `lane-logs/featB-r14.log`);
- **16 of 16 green** at load 6.5-9.3 (verifier's matched-load control,
  `lane-logs/verify-prefix-feat-summary.txt`).

Reported by the `tui-novel-reds-triage` lane's verifier as its D2 (one lifetime
observation, `verify-v-post-toggle3-r6.log`).

## Root cause

UNPROVEN. The hypothesis under test was that the click's Enter is view-local:
`TuiUserDriver::click_entity` is "navigate to the row, press Enter"
(`frontends/tui/src/user_driver.rs`), and on an `editable_text` row
`enter_pressed` resolves `EnterDecision::EnterEdit`
(`frontends/tui/src/app_main.rs`) — `engine.set_focus` in memory plus an editor
opened in `TuiState.edit_state`, no dispatched operation, hence no store write
and no CDC emission for `await_chord_settled` to observe.

Instrumentation refutes that this path is what reds: a probe on `click_entity`
reported `had_editor=false editor_opened=false` on **23 of 23** observed
invocations (`lane-logs/verify-probe-feat-r*.log`,
`lane-logs/verify-prefix-feat-r*.log`). Whatever stalls the emission tick under
load, it is not an editor that the click itself opened.

## Missing piece

A decodable mechanism. The barrier's failure message names only that the tick
did not advance; it does not say what the click dispatched, whether an
operation reached the store, or where the pipeline stopped. Until the barrier
reports that, load-correlated reds here cannot be separated from a real
CDC stall.

## Remedy

NOT FIXED. A `click_entity` change that took only the render barrier when the
press opened an editor was written and then removed: it was measured inert (the
new branch never executed) and the rate difference it appeared to buy was the
load difference above. The red is OPEN; the next step is enriching the
`await_chord_settled` failure payload so an occurrence is decodable.
