---
id: 2026-10-03-dense-patch-composed-run-misses-convergence
date: 2026-10-03
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  dense_patch a_patch_over_the_document_block_bound failed its seed settle because the settle
  had a fixed wall-clock budget and seeding a 2001-block page takes 20-120 s with load; the
  seed always finishes.
---

## Bug

`dense_patch_engine_exact a_patch_over_the_document_block_bound` fails in `converge_projections`
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs`): `projections did not reach a
combined fixed point within 30s`. Seen in the full core nextest run on main `03078ee3` and 1/8
alone at load 11. Found by the reds triage of the night of 2026-10-03.

A first fix scaled the budget to 40 ms per seeded block (80 s for 2001 blocks). The verifier of
that fix refuted it: the test failed again at `within 80.04s` in a full core run
(`scratchpad/night/verify-reds/full-core.log:4877`), and the worst of 6 isolated passes needed
76.66 s. The reds-fix-5 lane reproduced the 80 s failure 2/2 alone at load 92-150
(`main-reds/lane-logs/reds-fix-5/probe-load-{1,2}.clean`).

## Root cause

The settle after the seed `WriteOrgFile dense_page_0.org` (2001 blocks) had a fixed budget, and
the seed's work grows with machine load. The seed is not stuck:
- The ingest finishes in every measured run: `done: 2001 block(s) in 31-55 s`
  (`iso-baseline-1.clean`, `load-baseline-{1,2}.clean`, `probe-wide-{1,2}.clean`).
- The largest single step is one Loro full reprojection of 2001 creates (`reason=oversized`,
  which a bulk import needs): 18.7 s at load ~22, 22.7-33.9 s at load ~55-60.
- A temporary probe sampled every progress counter the settle can see (CDC watermark, feed seq,
  org tick, reactive epoch, Loro sync watermark, ingest counters, projection passes). The
  longest window with no change was 22-25 s at load ~130 in passing runs, and at least 54 s
  in the two runs that failed at 80 s.
- The product watchdog line `NO PROGRESS for 30s ... 2001 of 2001 block(s) done` that the
  verifier saw is a false stall report during this reprojection, not a hang. It is recorded
  as its own entry: `2026-10-03-ingest-watchdog-reports-a-stall-during-the-downstream-flush`.

The `[RESEED-ORACLE] FIRST steady-state full-reseed LEAK: reason=oversized` line is present in
both the failing and the passing runs. It is a harness artifact: the session marks the reseed
observer steady at its first transition, and here that transition is the bulk seed.

## Missing piece

A settle criterion for a bulk transition that tells a slow SUT from a stuck one. A fixed
budget, fixed or scaled by block count, cannot do it.

## Remedy

FIXED in the test harness. The seed settle now waits while the SUT makes progress:
- `convergence::converge_signals` takes a `Patience`. `Patience::WhileProgressing` gives up
  after `stall` with no signal and no work counter advancing, or at `ceiling`.
  `Patience::Budget` is the previous fixed budget, unchanged for every other caller.
- `ComposedSut::<WideE2E>::apply_bulk` settles with `BULK_SETTLE_STALL` = 120 s and
  `BULK_SETTLE_CEILING` = 360 s, and widens the outer wedge bound by the ceiling, so the settle
  reports before the wedge. The dense_patch session seeds through it.

Evidence (test profile, `main-reds/lane-logs/reds-fix-5/`):
- Alone: pass in 27 s (`new-iso-1.clean`); 3/3 pass at load 38-59 (`new-load-{1,2,3}.clean`).
- The whole `dense_patch_engine_exact` binary at load 55-167: 48/48 pass; this test took
  119.8 s, which the 80 s budget would have failed (`new-bin-load-1.clean`).
- Red with teeth: a temporary stub that never finishes the 2001-op reprojection fails the test
  at 129 s with `did not reach a combined fixed point after 129.3s: no signal or work counter
  advanced for 120s` (`red-hang-1.clean`). The stub was reverted.

Open: a hang inside one long step is found only after 120 s. A liveness counter in the Loro
full walk would allow a much shorter stall window (see the watchdog entry). Bulk ingest of a
2001-block page costs 31-55 s in the test profile; it needs a release measurement before it
can be judged as a performance defect.
