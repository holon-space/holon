---
id: 2026-11-03-shell-kill-group-returns-while-members-run
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, kill_group returned while members of the killed group still ran.
---

## Bug
Found by a code audit of the shell effect's restart path (card #872, defect N2); the tester's PBT went red on the base.

## Root cause
`kill_group` sent SIGTERM, slept, sent one SIGKILL and returned without a check that the group was gone. A member that was forking or exiting at the signal was still running at the return.

## Missing piece
No test checked that the group was gone at the return of `kill_group`; tests checked after a sleep.

## Remedy
crates/holon-net-shell/src/effect.rs `end_group` loops until no member of the group runs (a zombie counts as gone), with SIGKILL each round after the grace period. On macOS the members are read from the process table (crates/holon-net-shell/src/effect/process.rs `members`), because `kill(-pgid, 0)` gives EPERM for a group of zombies or exiting members. Red on the base: /Users/martin/.cache/armc/c872/logs/red-base.log (`kill_group_and_a_killed_run_return_once_no_member_of_the_group_runs`).
