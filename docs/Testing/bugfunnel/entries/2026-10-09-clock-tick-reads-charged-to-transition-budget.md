---
id: 2026-10-09-clock-tick-reads-charged-to-transition-budget
date: 2026-10-09
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The landing gate `just hand-authored` went red at random on tolerance-0
  pinned read budgets because the clock scheduler's 30 s tick read
  `clock_reader` inside whichever transition's measurement window was open,
  and the budget charged that read to the transition.
---

## Bug
Found by the ingest lane while running the landing gate on main 73e73daa
(`.claude/worktrees/agent-ab09e71426605ad86/lane-logs/ibr2-state.md`,
`lane-logs/ibr2/p31-*`): `OpenTabViaModifierClick.sql_reads: 24 exceeds expected
23 + tolerance 0` in `cmd-click-same-row-twice-activates`. The one extra read was
`SELECT DISTINCT grain FROM clock_reader`. The same read showed up in the
NavigateFocus, BulkExternalAdd, BlockToPage and SplitBlock windows of other runs.
No product defect: the scheduler does what it should; the measurement charged its
work to the wrong owner.

## Root cause
The clock scheduler ticks on a timer (`CLOCK_TICK_INTERVAL`, 30 s,
`crates/holon/src/di/registration.rs:60`) and each tick issues
`stored_reader_grains` (`crates/holon/src/sync/clock_scheduler.rs`). The keystone
counts every `query` span that finishes on the scope's threads between
`SpanCollector::reset` and `snapshot`
(`crates/holon-integration-tests/src/test_tracing.rs`, `snapshot`), with no notion
of who caused it. A tick that lands in a pinned window adds one read.

## Missing piece
No causal origin on the SQL a background timer issues, so a per-transition budget
could not tell a timer's work from the transition's.

## Remedy
Each scheduler pass now runs under a `clock_scheduler.tick` span marked with
`holon_api::periodic::mark_periodic`, which puts a `PeriodicTask` value in the
pass's OpenTelemetry context. Every descendant span inherits it; the test span
processor stamps `periodic_task` on those spans at start, and `snapshot` excludes
them from every metric (disclosed as `periodic_spans_excluded=N` on the
`[inv-sql-budget]` line). Budgets stay tolerance-0; the keystone keeps the real
scheduler at its real interval. Red-first proof:
`crates/holon-integration-tests/tests/span_capture_suite/periodic_reads_outside_budget.rs`
runs the real scheduler at 20 ms and asserts the window counts only its own read
(red: `left: 4, right: 1`, three `clock_reader` reads; green after).
