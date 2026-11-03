---
id: 2026-11-03-shell-first-open-forgets-unkilled-leftovers
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, the first pid-file write after Runner::open dropped the leftovers that open had reported, so a leftover the caller did not kill was lost to the next restart.
---

## Bug
Found by a code audit of the shell effect's restart path (card #872, defect S4); the tester's PBTs went red on the base.

## Root cause
`open` returned the leftovers but wrote a pid file with only this runner's runs. A leftover not killed before the next host death was then named by no file.

## Missing piece
No restart PBT restarted twice without a kill of the first restart's leftovers.

## Remedy
The runner keeps the reported leftovers (`Groups.left`); each write (crates/holon-net-shell/src/effect.rs `write_held`) re-checks them with `still_left` and writes those that still run, before the runner's own runs. Red on the base: /Users/martin/.cache/armc/c872/logs/red-base.log (fixed scenario "a leftover the first open kept is reported by the next open"; netd `netd_start_reads_every_pid_file_a_runner_writes_and_refuses_every_other_by_name`).
