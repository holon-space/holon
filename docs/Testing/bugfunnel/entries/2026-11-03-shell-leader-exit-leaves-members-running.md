---
id: 2026-11-03-shell-leader-exit-leaves-members-running
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, a run whose leader process exited on its own returned while other members of its process group still ran, and nothing tracked them any more.
---

## Bug
Found by a code audit of the shell effect's run path (card #872); the tester's PBT went red on the base.

## Root cause
`Runner::run` treated the leader's exit as the end of the run: it dropped the run from the pid file and returned. A member that the leader forked into the background kept running with no pid-file line, so neither the runner nor a later restart could end it.

## Missing piece
No test drew a leader that exits while a member of its group still runs, and none checked the group after `run` returned.

## Remedy
After the leader exits, crates/holon-net-shell/src/effect.rs `Runner::run` calls `end_group`, which sends SIGTERM, waits the kill grace, then SIGKILLs until no member of the group runs, before the pid file forgets the run. Red on the base: /Users/martin/.cache/armc/c872-r2/logs/red-base.log (`a_run_whose_leader_exits_returns_once_no_member_of_its_group_runs`).
