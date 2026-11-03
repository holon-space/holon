---
id: 2026-11-03-shell-dropped-run-orphans-group
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, dropping a running shell effect run (aborted task, runtime shutdown) left its process group running and still listed as in flight.
---

## Bug
Found by a code audit of the shell effect's run path (card #872); the tester's PBT went red on the base.

## Root cause
All cleanup of a run sat after the awaits in `Runner::run`. When the future was dropped at an await, none of it ran: the group kept running, `in_flight` kept listing it, and its spawn record stayed until the host died.

## Missing piece
No test dropped a run future after its child was spawned and then checked the group and the runner state.

## Remedy
A drop guard (crates/holon-net-shell/src/effect.rs `Unended` and its `Drop` impl) is armed right after the spawn. On drop it SIGKILLs the group until no member runs, removes the run from `in_flight` and deletes its spawn record; if the kill fails, the group moves to the runner's leftovers so the pid file keeps naming it. Red on the base: /Users/martin/.cache/armc/c872-r2/logs/red-base.log (`a_dropped_run_leaves_no_member_of_its_group_running_and_no_trace_in_the_runner`).
