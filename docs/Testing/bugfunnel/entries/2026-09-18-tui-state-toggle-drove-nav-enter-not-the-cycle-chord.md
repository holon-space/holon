---
id: 2026-09-18-tui-state-toggle-drove-nav-enter-not-the-cycle-chord
date: 2026-09-18
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The TUI driver had no state-toggle gesture, so `ToggleState` fell through to
  the trait default's "navigate then press Enter", which opens edit mode — the
  rung never toggled anything and panicked on its own re-click loop.
---

## Bug

`holon-tui::tui_ui_pbt` panics whenever a sequence draws `ToggleState`:

```
[toggle_state] re-click #1 failed for block:gen-10: await_chord_settled: engine
did not quiesce: emissions did not quiesce after dispatch — CDC pipeline stuck?:
emissions did not advance past tick 3 within 2s
  — crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:3801
```

Found by the `fix-drawer-open-known-red` lane's verifier (its
`verify-11-tui-fixed-1.log:42`), root-caused and fixed by the
`tui-novel-reds-triage` lane.

Engagement is the story: in an unbiased 36-run sweep exactly ONE case drew
`ToggleState` and it failed. Re-run with the generator biased
(`HOLON_PBT_WEIGHTS=ToggleState:400`), the rung failed **7 of 7** runs that
reached it (`lane-logs/wtoggle-r*.log`) — including draws where it was step 0.
The rung was not flaky; it was always broken, and the unbiased alphabet almost
never drew it.

## Root cause

`HeadlessFrontendComponent::toggle_state_via` drives the toggle through
`driver.cycle_state_toggle`, whose trait default is
`self.click_entity(entity_id, region)` — documented as faithful "for
window/geometry drivers whose hit-test lands on the actual glyph"
(`crates/holon-frontend/src/user_driver.rs:497-505`). The TUI has no mouse:
`TuiUserDriver::click_entity` is "navigate to the row, press Enter"
(`frontends/tui/src/user_driver.rs:633-651`), and Enter on a Block region
`enter_pressed` (opens edit mode) — it never cycles the task state
(`frontends/tui/src/app_main.rs:397-408`). So click #1 only moved focus, the
landing loop polled `block_task_state` for 500 ms, saw no change, and re-clicked
— and the repeat navigate+Enter on an already-focused, already-editing row
produced no CDC emission at all, making `wait_for_quiescence`'s "advance past
tick N" precondition unsatisfiable. The TUI's real toggle gesture is the
`cycle_task_state` key chord, Ctrl+T (`app_main.rs:258-276`), which the TUI's own
`tui_keystrokes_for_op` already maps (`user_driver.rs:376-379`).

No product defect: Ctrl+T toggles correctly in the TUI, and GPUI's
`click_entity` does hit the glyph.

## Missing piece

A TUI implementation of the trait's `cycle_state_toggle`. Every window driver
inherited a default whose stated precondition ("hit-test lands on the actual
glyph") the TUI does not satisfy, and nothing disclosed that — so the
`SutMutate`/`ToggleState` rung silently covered nothing in the TUI slice while
consuming generation weight, and the coverage loss looked like flakiness.

## Remedy

FIXED (`tui-novel-reds-triage`, second iteration — a first attempt was refuted
by the verifier and is why this section is longer than one line).

**Attempt 1 (WRONG, refuted).** Drive the TUI's real toggle gesture — the
`cycle_task_state` chord, Ctrl+T — through the input pipeline, the shape
`send_key_chord` uses. Under a `HOLON_PBT_WEIGHTS=ToggleState:400` sweep this
looked green (8/8) but the verifier found 10 state-toggle-family failures in 52
biased runs: 6 silent `inv-task-state-*` divergences (e.g. `block:c1:
ref=Some("DONE") sql=Some("") loro=Some("")` — the SUT advanced one cycle PAST
the reference), 2 verbatim pre-fix `re-click #1 failed … CDC pipeline stuck?`,
and 2 `click #N failed`. Three independent reasons it is unusable here:

1. `cycle_task_state` reads the prior keyword off the authority and advances one
   step, so it is NOT idempotent — while `toggle_state_via`'s landing loop
   re-clicks on the premise that stale re-clicks are no-ops
   (`components.rs:3765-3802`). Every re-click is a real extra cycle.
2. `nav_to` + a keystroke cannot be sequenced reliably: `nav_to` walks focus
   with Down/Tab and the app may still process a queued key when Ctrl+T arrives.
   Symptom: `nav_to: walked past target block:c1 (12 steps, max region size 5)`.
3. It replaced one hardcoded op→keystroke map lookup with another, so a
   keybinding change would not be caught — the verifier flagged this too.

Instrumentation pinned reason 1 (temporary probes, reverted with sha256 proof):
over 112 probed runs / 278 traced toggles `reclicks=0` in 110 and **the only 2
runs with a re-click are the only 2 that diverged** (2/2 vs 0/110). Detail in
`2026-09-18-toggle-landing-loop-assumes-an-idempotent-click-verb`.

**Attempt 2 (final).** `TuiUserDriver::cycle_state_toggle` now resolves the
glyph's dispatch from the resolved view tree and dispatches it, exactly as
`ReactiveEngineDriver` does. The view-computed verb IS idempotent under a stale
view, so the landing loop's premise holds; no `nav_to`, no keystroke, no op→
keystroke map. Disclosed cost: the TUI's Ctrl+T path is not exercised by this
rung (recorded in the idempotence entry above).

Evidence (all `lane-logs/`):

- pre-fix (no override) biased: 7/7 runs that reached the rung panicked
  (`wtoggle-r*.log`).
- attempt 1, biased: 10 toggle-family failures in the verifier's 52 runs
  (`verify-post-toggle*`, `verify-v-post-toggle*.log`).
- **attempt 2, biased: 68 runs, 0 toggle-family failures** — 20 serial at load
  10→91 (`wA-resolved-summary.txt`), 48 parallel at load 79→111
  (`wB-resolved-summary.txt`), against the verifier's refutation load of 43-60.
- the (b)/(c) seeds (4, 10, 12, 13, 28, 32) stay green (`final-seed-*.log`).

One consequence to note, absorbed by an existing registry row: with the recipe
now really in the windowed vault, a `SetEdgeField` draw can aim `set_field` at a
recipe step and the read-only gate refuses it — `op_write_cap.rs:513`,
`block/set_field(requires) on block:keystone-recipe.cook::b::0 failed: cooklang
is a read-only format` (2/48 at load 79-111). `scripts/keystone-known-reds.sh`
classifies it `known-red:cooklang-read-only-write-refusal-any-op` → PASS-WITH-
NOTE, so no gate blocks and no new row is needed.

Related, NOT fixed: `SutFocusWrite::apply_navigate_focus` fails the same way
(`components.rs:2881`, `emissions did not advance past tick 1 within 2s`) when
`NavigateFocus` targets `block:structural-page` — the block the windowed boot
seeds focus onto — so the click is a redundant re-navigation of the
already-focused row. Seen pre-fix at seed 16 and post-fix at seeds 1, 4, 7, 10,
and it is load-correlated: seed 1 failed 3/3 at load 13-51 and passed 6/6 at
load 7. Registered nowhere; it needs Martin's call on whether a no-op navigation
may legitimately emit nothing (tolerate it in the barrier) or must be excluded
by the generator.
