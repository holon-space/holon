---
id: 2026-09-01-main-panel-delivery-budget-load-sensitive
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: NOTED
summary: >-
  The cursor-filtered main-panel delivery budget fails only under full-suite
  parallelism — 853ms isolated vs 7.5s contended, an 8.8x inflation.
---

## Bug

`holon::turso_storage_repros
tabs_main_panel_delivery::cursor_filtered_main_panel_delivers_at_vault_scale`
failed in the main census run: "cursor-filtered main panel must deliver within
a couple of seconds at ~70-page vault scale; took 7.516280208s (unguarded
landmine form hung 60-90s)" (`lane-logs/ab-holon-main.nextest.log:1160`,
`FAIL [ 8.245s]`). Budget is `Duration::from_secs(5)`
(`crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:184`).

Found by orchestrator census, triaged in lane `reds-triage`.

## Root cause

**Not a product regression.** Run alone on the same tree the test delivers well
inside budget, three consecutive times
(`lane-logs/latency-iso.txt`):

```
=== isolated run 1 ===  [delivery] create+first-read = 859.623542ms (35 rows)
=== isolated run 2 ===  [delivery] create+first-read = 869.246416ms (35 rows)
=== isolated run 3 ===  [delivery] create+first-read = 853.700375ms (35 rows)
```

853–869ms isolated against 7.516s inside an 885-test nextest run — an **8.8x
inflation from CPU contention**, not from the query path. The test creates a
Turso IVM matview over ~1050 blocks and times creation plus first read with no
warm-up, so it is measuring a cold, CPU-bound step while nextest saturates every
core.

## Missing piece

A wall-clock budget assertion with no concurrency control. The test's own budget
is meaningful only on a quiet machine; nothing marks it as needing isolation, so
it reads as a product red whenever the suite is run in bulk.

## Remedy

Left unchanged deliberately — raising the budget to absorb contention would
destroy the guard's value (its prose records that the unguarded form hung
60-90s, so the budget is the whole point). The fix belongs with the gating
decision: if `-p holon` is gated, this test needs a nextest test-group that
pins it to limited concurrency, or a nightly tier that runs it unloaded.
Recorded so the next bulk run does not re-triage it as a latency regression.
