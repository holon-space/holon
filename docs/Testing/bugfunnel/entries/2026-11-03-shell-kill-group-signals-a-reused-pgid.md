---
id: 2026-11-03-shell-kill-group-signals-a-reused-pgid
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, Runner::open reported and kill_group signalled a recorded pgid that another process group had taken after a host restart, so a restart could kill a foreign group.
---

## Bug
Found by a code audit of the shell effect's restart path (card #872, defect item 7); the tester's state-machine PBT went red on the base.

## Root cause
The pid file named a group by its bare pgid. After the group ended, the OS could give the pgid to another group; `open` reported it as a leftover and `kill_group` sent it SIGTERM/SIGKILL.

## Missing piece
No restart PBT recorded a group, let the pgid change owner, and checked that the foreign group still ran after the restart.

## Remedy
A group is named by its pgid plus the start time of its leader (`Group.started`, pid-file line `[pgid, started, attempt, transition, subject]`). crates/holon-net-shell/src/effect.rs `identity` compares that start with the start the process table gives for the pid equal to the pgid (crates/holon-net-shell/src/effect/process.rs `start`):
- A leader with another start: `open` does not report the group, `kill_group` sends no signal.
- No leader (it exited, and members run): the group cannot be proved to be the recorded one. `open` (`still_left`) reports it without its start, as an unverified leftover that the pid file keeps; `kill_group` returns `ShellError::Unverified` and sends no signal. A dead host's own group whose leader exited is therefore reported, not killed.
- A run of a live host ends its own group also after its leader exited (`holds`): no other group can take the pgid while a member runs.
- Left open: between the check and the signal the group can end and its pgid be reused (no pidfd on macOS).

Red on the base: /Users/martin/.cache/armc/c872/logs/red-base.log (fixed scenario "a dead group's pgid that a foreign group took"). Red for a foreign group whose leader exited, before the leaderless case was refused: /Users/martin/Workspaces/pkm/holon/.claude/worktrees/lane-872-shell-no-orphan/target/verify-872/probe-identity.log (`kill_group_spares_a_foreign_group_whose_leader_exited`).
