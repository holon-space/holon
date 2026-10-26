---
id: 2026-10-26-netd-a-killed-effect-run-never-settles
date: 2026-10-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In netd, an effect run that its command timeout or an outside signal ended never settled, so `await` hung and `why_not` said in flight forever.
---

## Bug
The lane verifier found it with probes against a built netd. The net had an effect `/bin/sleep 30` with `command.timeout: 1s` and no `abandon` timer. After the fire, `await` never answered, and the input token left `marking`. An outside SIGKILL of the child gave the same result.

## Root cause
`run_effect` in crates/holon-netd/src/host.rs mapped `Ended::Killed` and `Ended::Signalled` to `None`. `drive` then sent no result to the executor, so the attempt stayed in flight. Only an `abandon` timer could settle it, and the format makes timers optional.

## Missing piece
No netd test net drew a run without an exit code. Every test effect exited on its own. Litmus: COVERAGE, because no test could generate the triggering interaction (a timeout kill). ORACLE does not apply, because `await` with a deadline would have flagged it.

## Remedy
A run without an exit code now settles as outcome unknown, with the cause and the log path. It may have done part of its work, so the transition's `receiver` decides. The pinning test is `an_effect_its_command_timeout_kills_still_settles` in crates/holon-netd/tests/netd.rs. NetHost.md and NetFileFormat.md say this.
