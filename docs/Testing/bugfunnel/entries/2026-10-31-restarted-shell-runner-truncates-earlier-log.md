---
id: 2026-10-31-restarted-shell-runner-truncates-earlier-log
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A restarted shell runner reuses attempt ids and truncated the log of the earlier runner's attempt with the same id, so the evidence of that run was lost.
---

## Bug
The tester replay (wave 14) found it on main 30b64bdd: a runner wrote
`Run-1.log` holding "first"; a runner reopened on the same pid file ran
attempt 1 again, and `Run-1.log` then held only "second". Card
holon-space/holon-work#871.

## Root cause
Attempt ids restart at 0 with each executor, and
`crates/holon-net-shell/src/effect.rs:347` (base 30b64bdd) opened the log
with `File::create`, which truncates an existing file (and follows a symlink
at that path).

## Missing piece
No test restarted a runner and reran an attempt id, and no test planted an
entry at a log path before a run.

## Remedy
`create_log` opens with `File::create_new` and, when the path exists, takes
the first free `<path>.N`; `Finished::log_path` names the file written.
Pinned by `a_restarted_runner_keeps_the_log_of_the_earlier_attempt` and the
G4/G5 state machines in `crates/holon-net-shell/tests/shell_log_paths.rs`.
