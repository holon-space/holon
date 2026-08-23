---
id: 2026-08-23-matview-lease-actor-stats-read-races-the-actor
date: 2026-08-23
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Under heavy machine load the matview_lease_actor tests read matview_stats()
  before the lease actor has recorded the grant, so a correct lease count is
  asserted as wrong.
---

## Bug

Two `holon-turso::matview_lease_actor` members failed during the
`repin-ecdbdae` lane (turso fork re-pin to `ecdbdae12b93`), each in a different
full-suite run, while the whole machine was thrashing behind a wedged sccache
server (suite wall time 9-18s instead of the usual ~5s):

* `only_the_last_release_reaps` — `crates/holon-turso/tests/matview_lease_actor.rs:106`,
  `assertion (stats.leased_views, stats.active_leases) == (1, 2)` failed with
  `left: (1, 1)`.
* `a_pin_outlives_a_whole_lease_cycle` — same suite, next run.

Found by agent exploration (a gate run in a re-pin lane), not by a product
test, so it is an escape by this ledger's definition. It is NOT attributable to
the re-pin — see Root cause.

## Root cause

The tests await `acquire_view_lease` and then read `db.matview_stats()`
immediately. Both tests are `#[tokio::test(flavor = "multi_thread")]` and the
lease bookkeeping lives in a separate actor, so the await returning does not
establish that the actor has already counted the grant. Under CPU starvation
the window between the two widens enough to be observed, and the test reports a
count that is merely NOT YET SETTLED as a wrong count. The assertion has no
bounded wait, unlike `inv-matview-consistent-with-recompute`, which rejects
transient maintenance lag by waiting for a stable fixed point.

Evidence that the pin is not the cause (`.lane-logs/`, all in the lane tree):

* `ab-lease-repeat.log` — the suite alone, 10/10 rounds green.
* `ab-lease-OLDPIN-load.log` — old pin `a95f1a81`, 30 rounds at
  `--test-threads 32`: 30/30 green, zero non-zero exits.
* `ab-lease-NEWPIN-load.log` — new pin `ecdbdae12b93`, the identical 30-round
  script: 30/30 green, zero non-zero exits.
* `g-turso-full.log` — one full-suite run on the new pin at 287/287 green,
  including both members that had failed.
* `ab-lease-OLDPIN.log` — 4 full-suite runs on the old pin, zero lease
  failures (the only failures were the 8 REPLACE/IVM tripwires, expected there).

A lease refcount read cannot be reached by an IVM REPLACE-path change, and the
failure did not reproduce on either pin once the machine was quiet.

## Missing piece

No bounded-wait settle between the actor's acknowledgement and the test's
observation of `matview_stats()`. The lease API hands back a grant before the
stats the test asserts on are guaranteed to reflect it, and no test-side helper
exists to wait for that. The secondary ORACLE tag records the consequence: the
assertion, as written, cannot distinguish "wrong count" from "count not yet
settled", so it is a false-positive generator rather than a defect detector.

Classified ENVIRONMENT rather than COVERAGE or PERCEPTION: the interaction is
generatable and was generated, and nothing here is visual, so the gap is the
timing divergence between the test's observation point and the actor's real
bookkeeping — the "async races the settle masks" case in the ENVIRONMENT row.

## Remedy

OPEN. Recommended: give the lease tests the same bounded-wait treatment the
matview invariants already use — poll `matview_stats()` to a stable fixed point
with a timeout, and fail loud on timeout — instead of reading once. Not done in
the re-pin lane: it is a behaviour change to a test outside that lane's scope,
and it should land with its own red-for-the-right-reason demonstration (the
current assertion made red deterministically by delaying the actor).
