---
id: 2026-10-09-degraded-bus-baseline-counts-the-harness-stuck-guard
date: 2026-10-09
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `shipped_window_launch_subscribes_to_the_degraded_bus_in_sql_only_mode`
  asserted the ConditionBus has zero subscribers before the window opens, but
  every TestEnvironment boot arms the database-stuck guard, which subscribes to
  that bus, so the baseline is 1 and the test was red on main with no product
  change behind it.
---

## Class

`FALSE-ALARM`: no product defect. The production window launch still subscribes
the degraded-bus bridge (`frontends/gpui/src/lib.rs`, `spawn_degraded_bus_bridge`).
The test asserted an absolute baseline (`subscriber_count() == 0`) that only the
shipped binary holds; the test harness adds its own subscriber.

## Bug

`holon-gpui --test degraded_bus_bridge_windowed
shipped_window_launch_subscribes_to_the_degraded_bus_in_sql_only_mode` failed
3/3 on main `73e73daa` and on integration at the pre-launch assertion
(`left: 1, right: 0`). Found by the orchestrator's windowed-test triage (lane
gpui-reds); there was no known-red row.

## Root cause

`TestEnvironment` arms the database-stuck guard in its first DI closure
(`crates/holon-integration-tests/src/test_environment.rs:492`,
`report_database_stuck_in`). The guard takes a broadcast receiver on the
session's ConditionBus (`crates/holon/src/testing/database_stuck_guard.rs:77`,
`bus.subscribe().changes`) and keeps it for the life of the bus. The guard was
armed ungated in every TestEnvironment boot by commit `431856497d3e`
(2026-10-04, hang detector round 8); from that commit on, the baseline is 1.

## Missing piece

No gate runs the windowed GPUI tests, so a harness change in
`holon-integration-tests` that breaks a windowed test in `holon-gpui` lands
green.

## Remedy

The test records the subscriber count before the launch and asserts the launch
raises it, and that it stays above the baseline after a condition is delivered
(`frontends/gpui/tests/degraded_bus_bridge_windowed.rs:74,93,112`). Teeth:
with the `spawn_degraded_bus_bridge` call disabled, the test goes red with
"1 subscriber(s) before the launch, 1 after".
