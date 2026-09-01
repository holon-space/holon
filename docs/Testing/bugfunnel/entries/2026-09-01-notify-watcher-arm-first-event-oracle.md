---
id: 2026-09-01-notify-watcher-arm-first-event-oracle
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `notify_watcher_delivers_events_after_arm` asserted on the first event off a
  freshly armed fsevents watch, so an unordered directory event failed it.
---

## The four gaps do not cover this one

Read the gap table before trusting the `gap:` field above. All four gaps
classify an **escape**: a defect that reached a human because some automated
layer failed to catch it. This is the opposite failure — **no product defect
existed**. The watcher delivered the `a.org` event in every run measured; the
test's own assertion raised a false alarm. ORACLE in particular would invert
its own definition ("no invariant would have flagged the defect"): the problem
here is an invariant that fires when nothing is wrong.

`gap:` is a required enum (`ENVIRONMENT | COVERAGE | PERCEPTION | ORACLE`), so
a value had to be chosen. ENVIRONMENT is the least-wrong of the four — the
skill lists "async races the settle masks" and platform divergence under it,
and the defect is exactly that the test's timing model does not match real
fsevents delivery. It is still a poor fit, and **this entry should be excluded
when the funnel distribution is used to rank test investment**, because
counting a false-alarm test as an escape skews the very number the funnel
exists to produce.

## Bug

`holon-filesystem`'s unit test `notify_watcher_delivers_events_after_arm` was
carried in the lane rules as a load-dependent flake on contradictory prior
measurements (one session recorded 10/10 passing isolated, another 0/5). Found
by a hygiene lane re-measuring it, not by a suite run.

**The old oracle is a genuine race — it is NOT deterministic**, and this entry
previously said otherwise. Measurements at base `ed38a4dae833`, on the
byte-identical base file:

| Who | Isolated runs | Passed | Logs |
|---|---|---|---|
| this lane, 1st block | 10 | 0 | `lane-logs/notify/iso-1.log` .. `iso-10.log` |
| this lane, 2nd block | 10 | 0 | `lane-logs/notify/base2-1.log` .. `base2-10.log` |
| verifier, fresh context | 5 | **2** | `lane-logs/verify/p1-1.log` .. `p1-5.log` |

25 isolated runs, 2 passes. Two separate 10-run blocks on this machine gave
0/10 each, but the verifier reproduced passes on the same file (restore proved
by sha256 `9278a383…` before and after), so "always fails" is wrong: the pass
is reachable and the rate is machine- and load-state-sensitive. The earlier
0/10-therefore-deterministic reading was a single-machine sample presented as a
fact — the same mistake that produced the contradictory priors it complained
about.

Suite-level, at base: **1 of 3 passed** — `lane-logs/notify/suite-1.log`
(`93 tests run: 92 passed (1 leaky), 1 failed`), `suite-2.log`
(`92 passed, 1 failed`), `suite-3.log` (`93 passed, 0 skipped`).

Every failure lands in ~0.12s against a 5s budget, so it is not a timeout.

## Root cause

The oracle, not the watcher. `crates/holon-filesystem/src/change_source.rs:880`
asserted `change.path.ends_with("a.org")` on the value of a **single**
`rx.recv()` — the first event off the broadcast channel. Arming a watch on a
fresh `tempfile::tempdir()` also delivers an event for the directory itself,
and fsevents does not order that against the subsequent `a.org` write, so
whichever arrives first decides the verdict. That ordering is what varies
between runs and machines, which is why the rate is unstable.

## Missing piece

An order-insensitive arrival oracle. The property under test is "an event for
`a.org` arrives within the budget after arming", but the assertion encoded the
strictly stronger and untrue "the *first* event after arming is for `a.org`".

## Remedy

`change_source.rs:876-896` now drains the channel until an `a.org` event
appears, bounded by the same existing 5s `tokio::time::timeout`, and asserts on
the timeout result with every path seen in the message. Re-measured:

- 10x isolated: **10 passed** — `lane-logs/notify/iso2-1.log` .. `iso2-10.log`,
  each `1 test run: 1 passed, 92 skipped`.
- 3x full crate suite: **3 of 3 green** — `lane-logs/notify/suite2-1.log` ..
  `suite2-3.log`, each `93 tests run: 93 passed, 0 skipped`.

Non-vacuity was checked independently by a verifier: pointing the oracle at a
file that is never written makes it consume the full budget and go red
(`lane-logs/verify/p2.log`, `Summary [ 5.139s] … 0 passed, 1 failed`).

## Keystone repro

Not attempted, and not applicable. This is a `holon-filesystem` unit test over
the `notify` watcher seam; `general_e2e_composed_pbt` drives no raw fsevents
watcher, so it has no path that reaches this assertion.
