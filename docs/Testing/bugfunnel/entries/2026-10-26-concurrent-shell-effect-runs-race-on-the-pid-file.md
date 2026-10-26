---
id: 2026-10-26-concurrent-shell-effect-runs-race-on-the-pid-file
date: 2026-10-26
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Concurrent shell-effect runs of one runner wrote the same temporary pid file outside the lock, so a run that succeeded could settle failed with "pid file ... unusable: No such file or directory".
---

## Bug
The lane verifier found it by running netd's own test `an_effect_fire_settles_by_the_exit_code_of_its_command` 40 times under load. It was red 4 times: a run of `/usr/bin/true` settled failed, with the cause `the shell effect's pid file ... is unusable: No such file or directory (os error 2)`.

## Root cause
`Runner::write_pid_file` in crates/holon-net-shell/src/effect.rs read the live groups under the lock and released it. It then wrote the shared `<pid file>.writing` and renamed it outside the lock. When two runs registered or forgot at the same time, both wrote the one temporary file, and the second rename found no file (ENOENT). In `register`, that error kills the new group, and in `forget` it fails a run that is done. The last rename could also carry a stale group list.

## Missing piece
Before netd, no host ran one runner's effects concurrently, and the tests of holon-net-shell ran one attempt at a time. Litmus: ENVIRONMENT, because the code path existed but the tests never ran it with concurrent runs. COVERAGE is secondary, because no test generated concurrent runs.

## Remedy
`write_pid_file` now holds the group lock until the rename. Writes are serialized, and the last write carries the latest groups. The pinning test is `runs_on_many_threads_at_once_all_settle_by_their_exit_code` in crates/holon-net-shell/tests/shell_effect.rs: 128 runs on 16 threads. It was red before the fix in target/net-host-cli-r1-red.log, and green 20 times in a row after the fix.
