---
id: 2026-10-08-root-layout-error-widget-while-boot-seed-runs
date: 2026-10-08
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  At boot the whole root layout rendered as an error widget ("perspective
  block:root-layout has no displayable panels") while the background seed was
  still writing the default layout's panels.
---

## Bug
Found by a verifier (c1-fix-verify, section D4). It ran
`frontend_slice_runs_no_error_widgets_over_real_render` concurrently with
itself. The test is green alone. Under load it fails with
`inv-viewmodel-snapshot: Root layout rendered as error widget`. On main
73e73daab0dc it failed 1 time in 24 runs (8 concurrent) and 9 times in 32
runs with info tracing (`lane-logs/root-layout/repro-main*/summary.txt`).

## Root cause
The boot seed writes the default layout one block at a time
(`crates/holon-app/src/seed.rs:108`). It runs in the background while the
root watcher already renders (`crates/holon-app/src/wiring.rs:791`). Between
the write of a panel and the write of its first source or render child, the
perspective has no displayable panel. `render_root_slot` turned that state
into a hard error (`crates/holon-api/src/perspective.rs:283`). The frontend
showed the error until the next structural change re-rendered the root.
Measurement: in all 9 failing traced runs, `render_entity('block:root-layout')
failed` occurs before `Seeded default layout`. In the 23 passing runs it
does not occur.

## Missing piece
In the test wiring the race occurs only under CPU load, so the keystone
frontend slice detected it only sometimes. Also, no state told a half-written
layout apart from a perspective that really has no panels.

## Remedy
`BackendEngine::layout_seed()` (`crates/holon/src/api/layout_seed.rs`) is
`Pending` from before the session exists until the boot seed returns
(`crates/holon-app/src/wiring.rs:730,741`; a guard settles it on drop). While
it is pending, a perspective without a displayable panel renders as `loading`
(`crates/holon/src/api/block_domain.rs:536`). After the seed settles, it is
still an error. The root watcher re-renders when the seed settles
(`crates/holon/src/api/ui_watcher.rs:184`), so the root does not stay on
`loading`.

Deterministic rung: `crates/holon-app/tests/root_slot_pending_while_seeding.rs`
stops the real seed after the left sidebar is written and before its first
child, and watches the root through the production `watch_ui`. Red
`lane-logs/root-layout/red.log` (root rendered `error`), green
`lane-logs/root-layout/green.log`. Teeth: without the render change
`lane-logs/root-layout/teeth-render.log`, and without the settle trigger
`lane-logs/root-layout/teeth-trigger.log`. After the fix, under the same load:
0 failures in 32 runs (`lane-logs/root-layout/repro-fixed-traced`).
