---
id: 2026-11-24-concurrent-shell-callers-move-the-net-clock-back
date: 2026-11-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Two threads that wrote to one net shell at the same time could hand the
  executor their times out of order, so the executor refused the later write
  with "the clock went back" and a caller that treats that write as infallible
  panicked.
---

## Bug

A run of `two_instance_composed_pbt` panicked in the device-key net with
`the device-key net takes a KeyToLoad token: the clock went back from 32.625µs
to 14.167µs`. Found by an automated run of the device-key work, not by a test of
the shell: no shell test had two callers at once.

## Root cause

`holon_net_shell::Shell` read its clock (`crates/holon-net-shell/src/lib.rs`:73)
before it took the executor lock. Thread A read t1, thread B read t2 > t1, B
took the lock first and advanced the executor to t2, then A handed it t1. The
executor refuses a time earlier than its last one
(`crates/holon-net/src/exec.rs` `advance`), so A's write failed. Every shell
user with more than one caller had the race. The device-key net hit it because
the boot thread writes the first token while the net's driver task, spawned
just before, already steps the same shell.

Litmus, COVERAGE: no shell test generated concurrent callers, so the
interleaving was never drawn. Litmus, ORACLE: a run that drew it would have
failed, because the executor already refuses a clock that goes back.

## Missing piece

No test of the shell wrote from more than one thread.

## Remedy

The shell reads the clock under the executor lock (`at_now`,
`crates/holon-net-shell/src/lib.rs`:86), so the times the executor sees follow
the lock order. The stress test `writes_from_many_threads_never_move_the_clock_back`
writes from many threads at once and is red when the clock is read before the
lock again.
