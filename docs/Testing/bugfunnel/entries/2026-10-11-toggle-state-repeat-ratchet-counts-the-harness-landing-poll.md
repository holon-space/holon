---
id: 2026-10-11-toggle-state-repeat-ratchet-counts-the-harness-landing-poll
date: 2026-10-11
gap: FALSE-ALARM
secondary: null
status: OPEN
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
Open, queued. Either keep harness reads out of the ratchet (drop
`origin=<no-parent>` reads that the harness issued), or make
`await_toggle_landed` wait on the projection's change signal instead of a 10 ms
SQL poll. The slow landing itself (1.3 s for five clicks) is the lane's repair
path and is judged by `inv-settle-budget`, not by this ratchet.
