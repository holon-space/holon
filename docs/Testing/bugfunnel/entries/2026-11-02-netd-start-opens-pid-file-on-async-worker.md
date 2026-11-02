---
id: 2026-11-02-netd-start-opens-pid-file-on-async-worker
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  netd start read, wrote and renamed each net's pid file on the async worker, so a slow pid-file read or write stopped every task on that worker (403 ms on a 400 ms hold window).
---

## Bug
Found in review of the fix that moved the run-path pid-file writes off the
async worker: that fix left the open at start on the worker. A latency escape
(interaction SLO 200 ms), not a crash.

## Root cause
`start` (`crates/holon-netd/src/lib.rs:114`) is async and called
`HostedNet::load` (`crates/holon-netd/src/host.rs:50`) directly. The load calls
`Runner::open` (`crates/holon-net-shell/src/effect.rs:134`), which takes the
process-wide pid-file write lock, reads the pid file and writes and renames it
(`write_held`), all blocking. With the read or the write held by a test FIFO,
a heartbeat task on a current-thread runtime waited the whole window:
"a task on the runtime waited 403.256791ms (bound 200ms) while netd start held
a pid-file Read".

## Missing piece
No test held the pid-file I/O during netd start; the held-write tests covered
only the run path (`in_flight`, `abandon`).

## Remedy
`start` runs each `HostedNet::load` through `tokio::task::spawn_blocking`.
The PBT `a_task_on_the_runtime_runs_within_the_bound_while_netd_start_waits_for_a_held_pid_file_write`
(`crates/holon-netd/tests/netd_start_pid_file.rs`) holds the read or the write
at start and bounds the largest heartbeat gap by `BOUND` = 200 ms; it is in the
timing-critical group (docs/Testing/TimingCritical.md).
