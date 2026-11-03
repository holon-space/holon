---
id: 2026-11-03-shell-host-killed-before-record-orphans-group
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, a host killed between the spawn of a run and its pid-file record left a running group that no restart found.
---

## Bug
Found by a code audit of the shell effect's restart path (card #872, defect item 3); the tester's PBT went red on the base.

## Root cause
The pid file got the group only after the spawn returned. A host killed in that window left a group that no file named.

## Missing piece
The restart PBT never killed the host between the spawn and the record.

## Remedy
Before the spawn, crates/holon-net-shell/src/effect.rs `spawn_record` creates a file next to the pid file that names the run; the child writes its pid and start into it before exec (`spawn`, its `pre_exec` hook). `register` removes it after the pid-file write. `Runner::open` reads the records of hosts that no longer run (`orphan_records`), reports their live groups, writes them into the pid file and then deletes the records. Red on the base: /Users/martin/.cache/armc/c872/logs/red-base.log (`a_restart_reports_every_live_group_a_dead_host_left`).
