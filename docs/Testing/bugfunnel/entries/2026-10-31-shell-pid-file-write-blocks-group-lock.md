---
id: 2026-10-31-shell-pid-file-write-blocks-group-lock
date: 2026-10-31
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A slow pid-file write of the shell effect runner blocked `in_flight` and `abandon` and the async worker for as long as the write took (604 ms in the probe).
---

## Bug
The tester replay (wave 14) found it on main 30b64bdd with a probe that put a
FIFO at the pid file's temporary path: `in_flight` waited 604 ms behind the
held write. Card holon-space/holon-work#874. Latency above the SLO.

## Root cause
`Runner::write_pid_file` (`crates/holon-net-shell/src/effect.rs:234`, base
be66d1fb) held the group lock from the read of the groups through
`fs::write` (`:250`) and `fs::rename` (`:251`). `register` (`:211`) and
`forget` (`:223`) called it inside the async `run`, so the blocking write ran
on the async worker as well. `create_log` (`:150`) also ran on the worker.

## Missing piece
Litmus COVERAGE: no scenario in the net-shell tests reached the state "pid-file write held while `in_flight` is called"; the fix added scenarios, so the primary gap is COVERAGE.
Litmus ORACLE: no 200 ms latency bound existed on `in_flight`/`abandon`, so a case that hit the state would also have stayed green; that is the secondary gap.

No test held a pid-file write and measured `in_flight`, `abandon` or the
runtime meanwhile.

## Remedy
The pid-file write and the log creation run on the blocking pool. A separate
write lock, one per pid-file path in the process, orders the writes; the
group lock is held only to copy the groups and to mark them filed after the
rename. `in_flight` and `abandon` see a group only once a rename carried it,
and a failed register write kills its group inside the blocking task.
Pinned by `in_flight_abandon_and_the_runtime_answer_within_the_bound_while_a_pid_file_write_is_held`
(`crates/holon-net-shell/tests/shell_pid_file_held_write.rs`, bound 200 ms,
red on the base at 207 ms) and
`a_restart_reads_exactly_the_groups_that_still_run_after_any_sequence_of_runs`
(`crates/holon-net-shell/tests/shell_pid_file_restart.rs`).
