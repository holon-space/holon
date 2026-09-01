---
id: 2026-09-01-loro-restart-flip-ignored-invariant-10-epoch
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The Loro-enable-over-a-populated-SQL-vault restart test booted straight into
  a consolidator flip, which production refuses under Model.md invariant 10, so
  the test asserted an upgrade path that is explicitly unbuilt.
---

## Bug
`holon-integration-tests::loro_suite
loro_restart_unseeded_vault::restart_with_loro_enabled_over_populated_sql_vault`
was RED on `main` itself (base `73a6c3e3`, and already red at `d49ef031`):

```
panicked at crates/holon-integration-tests/tests/loro_suite/loro_restart_unseeded_vault.rs:110:10:
phase-2 start_app over the same test.db must succeed: Model.md invariant 10
(consolidator handover is an epoch, not a runtime lookup) violated: the
persisted consolidator is `direct` but this process is configured for
`projected`.
```

Found by the `loro-reds` lane, same run as
[[2026-09-01-loro-projection-stub-sink-stale-edge-columns]].

## Root cause
The test models the real upgrade story: populate a vault with Loro OFF, stop,
flip `[loro] enabled = true`, reopen the same `test.db`. Phase 1 therefore
persists a `direct` consolidator marker, and phase 2 configures `projected`.

That flip is exactly what `guard_consolidator_epoch`
(`crates/holon-app/src/consolidator_epoch.rs:102-113`) refuses. Model.md
invariant 10 (`docs/Architecture/Model.md:91-104`) states the reason — bases
are only meaningful against one consolidator's linear history — and states that
the state-preserving handover migration is **unbuilt** (spec 0008 Phase 4.1).
Today's only supported handover is the operator acknowledgement
`HOLON_CONSOLIDATOR_MIGRATE=1`, which wipes every component's durable state
(Turso db + CRDT dir) so the new consolidator re-seeds from the surviving vault
org files.

So the guard is right and the test's premise was stale: it asserted an
upgrade contract (phase-1 SQL state survives the flip intact and is adopted)
that the ratified invariant says does not exist yet. Nothing had run the suite,
so the mismatch was never surfaced.

## Missing piece
The test's environment diverged from production at the boot path: production
refuses an unacknowledged consolidator flip, and the test never acknowledged
one — it simply expected the boot to succeed. No `just` recipe or land gate ran
`--test loro_suite`, so the red sat on `main` and was allowlisted.

## Remedy
FIXED — and the escape route is closed at the gate: `just loro-suite` was added
AND wired into `just landing-gate` as an explicit step (`landing [8/10]: loro
consolidator suite`, justfile:1136-1137). The recipe existing was not enough;
nothing called it, so "no gate ran loro_suite" would have stayed true.

Phase 2 now drives the disclosed interim handover: it sets
`HOLON_CONSOLIDATOR_MIGRATE=1` for that boot only (nextest gives each test its
own process), then asserts the migration's real semantics — the wipe removes
the Turso db, and the phase-1 blocks come back re-ingested from the org files
the wipe left behind, **under their authored ids** (measured, not assumed: the
polling re-ingest assertion passes). The split/tree assertions the test exists
for are now reached for the first time. The module doc states the epoch
contract instead of the unbuilt one.

A settle race was found and fixed alongside it (see below); it had to be, because
its symptom masqueraded as teeth. Fixing the epoch premise alone left the test
flaky: `nodes_before` was sampled as soon as the TARGET block appeared in the
Loro tree, but the org-scan re-seed walks in document order and was often still
adding nodes, so the split's "+1 node" assertion measured the seed's tail
(`left: 10, right: 9`, `lane-logs/gateA-loro.1788252478.log`). Phase 2 now
baselines the count only after it holds still across three consecutive 200ms
samples plus `wait_for_loro_quiescence`. The assertion itself is unchanged —
this is a barrier, not a relaxation. Stability after the fix: 7/7 green
(`lane-logs/stab.1788253430.log`).

Teeth (production inversion, `lane-logs/red-teeth-nowipe.1788254428.log`):
making `guard_with_migrate`
(`crates/holon-app/src/consolidator_epoch.rs:89`) ignore the acknowledgement
(`if migrate && false`) turns the restored test red 3/3 for the right reason —
the invariant-10 bail arriving at the phase-2 boot `expect`
(`loro_restart_unseeded_vault.rs:126`). So the test does drive the production
epoch guard through the real boot path. The inverted file was restored
byte-for-byte (sha256
`935a32f6d1c9b08674262564989e6f0530daa7047a5fbe625df226c2f54ae869`).

**Negative result, recorded honestly.** A first inversion — making
`wipe_durable_state` (`crates/holon-app/src/consolidator_epoch.rs:155`) a no-op,
so the flipped boot runs over the stale `direct`-epoch database — did NOT turn
the test red once the settle race was fixed (3/3 pass,
`lane-logs/red-teeth-nowipe.1788253488.log`). An earlier run of that inversion
appeared red with `left: 10, right: 9`, but that was the settle race, not the
inversion. **The test therefore does not pin the wipe**: it proves the flip is
refused unless acknowledged, not that the acknowledgement actually clears the
stale durable state. That is a residual ORACLE gap, left OPEN for a follow-up
(assert the db file is unlinked / that no `gen_key_between` sort_key survives
into the Loro-fi keyspace).

Keystone repro: no. The composed keystone has no restart transition and no
consolidator flip — that absence is why this twin exists (the test's own
`@pbt overlaps` note says so). Closing it in the keystone would mean an
in-sequence app restart transition, which is not planned.
