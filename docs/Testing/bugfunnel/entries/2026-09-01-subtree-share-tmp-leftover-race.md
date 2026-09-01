---
id: 2026-09-01-subtree-share-tmp-leftover-race
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A share write's `.loro.tmp` is still on disk when `subtree_share_round_trip_pbt`
  sweeps for leftovers, so the atomic-rename publish is not atomic end to end.
---

## Bug

`sync_pbt::tests::share_subtree_pbt::subtree_share_round_trip_pbt` (target
`sync_suite`) intermittently fails its `P-NO-TMP-LEFTOVER/B` property. Found by
a hygiene lane measuring the test's isolated rate, not by a suite run.

Decoded payload, `lane-logs/flakes/subtree-1.log:81`, panic at
`crates/holon/tests/sync_suite/sync_pbt.rs:803`:

```
P-NO-TMP-LEFTOVER/B: stale tmp files:
["/var/folders/.../T/.tmpMjfFmI/shares/9cc1d93a-a764-463e-b88d-436458a594f4.loro.tmp"]
```

Re-panicked by proptest at `sync_pbt.rs:1142` as
`Test failed: failed in other process.` with:

```
minimal failing input: actions = [
    CrossPeerSyncAfterRestart(" [X:wsi]"),
    SettleSaves,
    EditOnA(" [A:tx]"),
    EditOnA(" [A:l]"),
]
successes: 7
```

Rate, measured 2026-09-01 at base `ed38a4dae833`, 10 isolated runs (one
`cargo nextest run … -E 'test(subtree_share_round_trip_pbt)'` per run, each
tee'd to its own log, serialized through the `holon-build` semaphore):
**9 passed, 1 failed**. The failure is `lane-logs/flakes/subtree-1.log`
(`Summary [ 43.621s] 1 test run: 0 passed, 1 failed, 7 skipped`);
`subtree-2.log` .. `subtree-10.log` each report
`1 test run: 1 passed (1 slow), 7 skipped`.

One failure in ten is ONE sample. Do not read it as a 10% rate.

## Root cause

NOT ESTABLISHED. What is measured: it reproduces in isolation, so it is not an
artifact of suite-level machine contention. Wall time across the ten runs
swings 43s–309s and the run that failed is the SHORTEST of the ten, which is
consistent with a timing-sensitive publish rather than with a particular drawn
action sequence. The leftover is a `.loro.tmp` under `shares/`, i.e. the
temp-file half of a write-then-rename publish: either the rename had not
completed when the property swept, or the temp file was orphaned by a path that
never renames it.

The minimal input above is recorded for one-paste re-arming; it has NOT been
replayed deterministically, so it is a lead, not a repro.

## Missing piece

A settle point the property can wait on. `P-NO-TMP-LEFTOVER/B` sweeps the
`shares/` directory at a moment the test picks, with nothing tying that moment
to "every publish this action list started has finished its rename". That is an
ENVIRONMENT gap in the skill's sense — the harness's timing model diverges from
the real publish path — but note that until the mechanism is established it is
equally possible the publish itself is genuinely non-atomic, which would make
this a product defect rather than a harness one. The classification should be
revisited once the mechanism is known.

## Remedy

OPEN. Deliberately NOT registered in `docs/Testing/KeystoneKnownReds.md`: a
`known-red` row there is consumed section-blind by
`scripts/keystone-known-reds.sh:50-55` and would auto-demote this signature to
a WARN pass-with-note in composed nightlies, hiding a suspected real race.

Next step is to establish the mechanism — instrument the publish path to record
rename completion, then decide between a harness settle point and a fix to the
publish itself. Needs an owner.

## Keystone repro

Not attempted. This is a `holon`-crate `sync_suite` property over the share
publish path; `general_e2e_composed_pbt` has no share/publish transition in its
catalog, so it could not draw this interaction as things stand.
