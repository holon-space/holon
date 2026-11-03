---
id: 2026-11-03-shell-unrecorded-run-keeps-members-running
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, a run whose pid-file record failed got one SIGKILL, and a member that its shell forked at that moment kept running with no record.
---

## Bug
Found by a code audit of the shell effect's register path (card #872, defect item 4); the tester's PBT went red on the base.

## Root cause
When the pid-file write failed, `register` sent one SIGKILL to the group and returned the error. A child that was being forked when the signal arrived is not in the group's member list yet, so the signal misses it (measured on macOS: a group killed during a fork keeps the new child).

## Missing piece
No test made the pid-file write fail while the run's shell forks.

## Remedy
When `register` (crates/holon-net-shell/src/effect.rs `Runner::register`) returns the pid-file error, the run's drop guard `Unended` kills the group (`kill_now`): SIGKILL each millisecond until no member runs. The unreaped leader keeps the pgid from reuse meanwhile. Red on the base: /Users/martin/.cache/armc/c872/logs/red-base.log (`a_run_the_pid_file_cannot_record_leaves_no_member_running`).
