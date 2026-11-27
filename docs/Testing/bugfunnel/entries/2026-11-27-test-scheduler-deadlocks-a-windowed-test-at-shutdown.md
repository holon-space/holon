---
id: 2026-11-27-test-scheduler-deadlocks-a-windowed-test-at-shutdown
date: 2026-11-27
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A windowed GPUI test hangs in `App::shutdown` until the nextest cap kills it:
  the gpui fork's test scheduler takes its two locks in opposite orders on the
  test thread and on a thread that wakes a foreground task.
---

## Bug
Found by the windowed gate: `vault_loading_placeholder_windowed` hit the
nextest TIMEOUT now and then, with no message. A hang loop on that binary
caught one hang after 147 clean runs; two `sample` stacks 30 s apart showed
the same frozen threads.

## Root cause
- The test thread runs `App::shutdown` →
  `ForegroundExecutor::block_with_timeout` → `TestScheduler::block_until`.
  With a timeout, `block_until` locked `rng` and then `state` to draw its
  tick budget.
- A tokio worker of the reactive engine wakes a foreground task at the same
  time: `TestScheduler::schedule_local` locks `state` and then `rng` (with
  `randomize_order`, the GPUI test default).
- Each thread holds the lock the other one waits for. Every windowed test
  that shuts the app down while a background thread can wake a foreground
  task has the same path.

## Missing piece
- Environment: the fork's test scheduler is test-only infrastructure, so no
  production run ever reaches this path; only the windowed tests do.
- Coverage: the fork's scheduler tests never wake a foreground task from
  another thread while the scheduler thread is inside `block_with_timeout`,
  so the opposite lock order went unseen.
- The windowed test itself could not name the stuck step: a deadlocked step
  cannot fail itself, and the nextest TIMEOUT names nothing.

## Remedy
- The fork's `block_until` reads `timeout_ticks` under `state` before it
  locks `rng`, so both paths take `state` first. A fork scheduler test wakes
  a foreground task from another thread in a loop while the scheduler thread
  calls `block_with_timeout`; with a yield injected between the two lock
  takes of the old order it deadlocks, with the fix it passes.
- The windowed test runs `pump` and `shut_down` under a watchdog: a step that
  does not return within the hang deadline prints its name and the stacks of
  every thread, and aborts before the nextest cap.

## Litmus
- ENVIRONMENT, "Does the failing code path even run in the keystone's
  wiring?": no. The headless keystone never runs the gpui test scheduler's
  `block_with_timeout` while a reactive-engine thread wakes a foreground task;
  only windowed tests at shutdown do.
- COVERAGE, "Is there a transition sequence in the current catalog+wiring
  that reaches this state?": the interaction (quit while rows still stream)
  is generatable; what no test drew is the cross-thread wake inside the fork
  scheduler's `block_with_timeout`. Secondary: COVERAGE.
