---
id: 2026-09-18-tui-focus-walk-barriers-on-any-render
date: 2026-09-18
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The TUI driver's `nav_to` barriered each navigation key on "a render happened"
  rather than "focus moved", so under projection lag it pressed Down again before
  the previous press was visible, skipped over the target row, and failed with
  `nav_to: walked past target`.
---

## Bug

While putting the TUI back on the real `cycle_task_state` chord (ruling D143.a,
`2026-09-18-toggle-landing-loop-assumes-an-idempotent-click-verb`), the driver's
focus walk failed before the chord was ever pressed:

```
[toggle_state] click #1 failed for block:parent: nav_to: walked past target
block:parent (12 steps, max region size 5)
  — lane-logs/diagC-r1j4.log
```

It was load-correlated: rare at the loads the previous lane measured, common at
the ~250-300 the machine carried during this work. In the round-1 shape **22 of
72 runs** carry the signature — 17 of them on a re-click, the rest on a first
click (`nav_to_walked_past` for `redC` in `lane-logs/rederive-figures.txt`; one
such log is `lane-logs/redC-r1j1.log`). Across the 82 shipped-arm runs it is
**0**.

## Root cause

`TuiUserDriver::nav_to` (`frontends/tui/src/user_driver.rs`) presses Tab or Down
and then awaits `await_render`, which returns as soon as `render_seq` advances
past a value read AFTER the press. Any render satisfies it — including a
CDC-driven re-render unrelated to the key. The loop's next iteration then reads
`focus_index`, which the renderer has not yet updated for the press that just
went out, sees a stale value, and presses again. The read stays a step behind the
walk, so over a 5-element region the walk can miss the target on every lap and
bail at the `2 * max_seen + 1` bound. Instrumented (`[probeN]`, reverted with
sha256 proof), the press/read divergence is visible as a frozen focus index:

```
[probeN] step=0 focus_index=5 focused=Some("block:structural-page") region_size=5 target=block:c2
[probeN] step=1 focus_index=5 focused=Some("block:structural-page") region_size=5 target=block:c2
[probeN] step=2 focus_index=5 focused=Some("block:structural-page") region_size=5 target=block:c2
[probeN] step=3 focus_index=5 focused=Some("block:structural-page") region_size=5 target=block:c2
  — lane-logs/diagN-r1j*.log
```

Four Down presses with the focus index unmoved: each one is a duplicate of a move
the app had already been asked for. A walk that ends one press past its target
also seats focus on the WRONG row, which is how an advancing verb (`Ctrl+T` acts
on the focused block) can cycle a block the reference never touched.

## Missing piece

Any check that the press did what it says. The barrier asserted "the renderer ran
a cycle", and no probe consulted the app's own focus oracle
(`focus_index`/`last_registry`, both written by the TUI state) to confirm the move
landed. Nothing in the biased alphabet can request a lagging projection either,
so no rung asked for the trigger.

## Remedy

FIXED (`fix-toggle-verb-nature`). `TuiUserDriver::step_focus(key, deadline)`
snapshots `focus_index` before the press and waits until it differs, failing loud
at the deadline otherwise; both the Tab region-hop and the Down walk now go
through it. Measured: 8 runs at load 250 → 0 `walked past`, and the walk's maximum
observed step count dropped from the 2*max+1 bound to 1-2
(`lane-logs/diagS-r1j*.log`).

Gap class COVERAGE: the trigger is a projection lag no transition, precondition or
weight can request — the same shape as the sibling entry's re-click trigger.
