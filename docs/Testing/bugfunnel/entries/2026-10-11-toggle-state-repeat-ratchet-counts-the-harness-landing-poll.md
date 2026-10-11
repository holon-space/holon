---
id: 2026-10-11-toggle-state-repeat-ratchet-counts-the-harness-landing-poll
date: 2026-10-11
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `inv-sql-budget` failed a `ToggleState` with `sql_read_repeat` 75 > 64 for
  `SELECT json_extract(properties, '$.task_state') … FROM block_raw WHERE id = …`,
  but those 75 reads are the keystone harness's own 10 ms poll waiting for each
  click to land, not a production consumer.
---
## Bug
Found by the keystone on the places-experiment lane (holon-space/holon-work
#164, workspace places-exp2), not on main. A five-click `ToggleState` on the
lane's fixture block went red:
`ToggleState.sql_read_repeat: one binding-set of SELECT json_extract(properties,
'$.task_state') AS task_state FROM block_raw WHERE id = … re-executed 75x, over
the redundancy ratchet 64`. The same run printed `reads=143 (dedup 24)/26` and a
`settle_budget` wall time of 1297 ms (within the machine-load slack). Another
run printed 73x. Log: places-exp2 `target/places-exp2-run-toggle-sql-budget.log`.

## Root cause
The repeated text is exactly the harness reader
`HeadlessFrontendComponent::block_task_state`
(crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:3602), and
the trace shows it with `origin=<no-parent>` (no production span above it).
`await_toggle_landed` (same file, ~4545) calls it every 10 ms until each click
changes `task_state`. So the repeat count is (time a click takes to land) /
10 ms, summed over clicks. The lane's toggle carries extra repair writes (6
writes against 3 expected), so its clicks land slower and the poll count passes
64. The production reader with the same text
(`block_task_state_by_id`, crates/holon/src/api/query_engine.rs:330) has no
caller.

## Missing piece
The redundancy ratchet (`MAX_READ_REPEAT_PER_BINDING`,
crates/holon-integration-tests/src/pbt/transition_budgets.rs:369) counts every
traced read in the transition window, including the harness's own polls. Its
purpose is to catch production re-execution growth.

## Remedy
Fixed: harness observation reads are kept out of the measurement, structurally
and not by raising a number. `test_tracing::harness_probe` marks a future with
a `HarnessProbe` OpenTelemetry context value — the same mechanism
`holon_api::periodic::mark_periodic` already uses for background timers — so
`ScopeRoutingProcessor::on_start` stamps `harness_probe` on every span under it
and `SpanCollector::snapshot` partitions those out of every metric. The two
harness SQL read surfaces run under it:
`HeadlessFrontendComponent::sql_query` and `SqlProjectionComponent::query`.
The exclusion is disclosed on the `[inv-sql-budget]` line as
`harness_probe_spans_excluded=N`, never silent.

A production read that repeats is unaffected: it carries no marker, so the
ratchet still sees it. Both halves are pinned by
`tests/span_capture_suite/harness_probe_reads_outside_budget.rs` — 75 probed
polls charge the window 0 reads, the same statement unprobed 75x still reports
`worst_read_repeat` 75.

The slow landing itself (1.3 s for five clicks) is the lane's repair path and is
judged by `inv-settle-budget`, not by this ratchet.
