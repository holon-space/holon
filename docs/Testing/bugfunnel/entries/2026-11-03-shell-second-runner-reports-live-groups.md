---
id: 2026-11-03-shell-second-runner-reports-live-groups
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, a second runner opened on a pid file that a live runner still used reported that runner's running groups as leftovers and overwrote its pid-file lines.
---

## Bug
Found by a code audit of the shell effect's open path (card #872); the tester's PBT went red on the base. A netd start dropped while its open waited makes this case in production: its late open runs beside the next start's runner.

## Root cause
`Runner::open` had no notion of ownership: any open read the pid file, reported every running group in it as left by a dead runner, and rewrote the file with its own view, so a caller could kill a live runner's groups and a later restart could miss them.

## Missing piece
No test opened a second runner, in this or another process, while the first one still ran groups.

## Remedy
`Runner::open` first takes an exclusive flock on `<pid file>.lock` (crates/holon-net-shell/src/effect.rs `own`) and holds it for the runner's life; while another runner holds it, open waits up to 1 s and then returns `ShellError::InUse` naming the holder's pid, with no read and no write. Pid file names must end in `.pid` (`ShellError::PidFileName`), so no lock, temp or spawn-record file is another pid file. holon-netd refuses a duplicate net name before it opens the pid file. Red on the base: /Users/martin/.cache/armc/c872-r2/logs/red-base.log (`a_second_runner_on_a_held_pid_file_is_refused_and_changes_nothing`, `pid_files_with_distinct_names_in_one_directory_are_independent`).
