---
id: 2026-10-03-dense-patch-composed-run-misses-convergence
date: 2026-10-03
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  dense_patch a_patch_over_the_document_block_bound failed its 30 s settle because seeding a
  2001-block page takes 30-47 s in the test profile under load; the projections do converge.
---

## Bug

`dense_patch_engine_exact a_patch_over_the_document_block_bound` fails in `converge_projections`
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:243`): `projections did not reach a
combined fixed point within 30s`. Seen 1/1 in the full core nextest run on main `03078ee3` (night
triage `main-reds/full-1.log:4088`) and 1/8 alone at load 11 (`main-reds/iso/B-6.log:119`). Found
by the reds triage of the night of 2026-10-03. Measured on tree `32b0c5b2`, test profile, no
release run (`b2-receiver-click/lane-logs/b3/b3-report.md`).

## Root cause

The settle that fails is the one after the seed `WriteOrgFile dense_page_0.org` (2001 blocks).
The seed's real work does not fit in the 30 s `CONVERGE_BUDGET` under load:
- 16/20 runs failed at load 12-140; runs at load 12-22 passed (`b2-receiver-click/lane-logs/b3/probe.summary`).
- All 17 failing runs reached the fixed point late, at 30.1-46.6 s. No run was still active at
  +150 s, so there is no loop.
- Time split of one failing run at load 48 (`b2-receiver-click/lane-logs/b3/timed-1.log`): `boot_parse` 7.9 s
  (:290); Loro->SQL full projection of 2001 creates 21.2 s, of which ~19.7 s is the sink apply at
  ~10 ms/row (:308); org write-back ~9 s, `home_by` fold ~5 s and render ~4 s (:313-323); fixed
  point at 37.9 s (:324).
- Each failing run has exactly one full projection walk (`reason=oversized`), which a 2001-row
  bulk import needs. After the write-back neither Loro, CDC nor the feed moved again.

The `[RESEED-ORACLE] FIRST steady-state full-reseed LEAK` label is a harness artifact: the
session marks the reseed observer steady at its first transition
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:2218`), and here that transition is
the bulk seed.

The second reseed reason seen once (`empty_pending_moved_frontier`, `main-reds/iso/B-6.log:116`,
0/21 runs on `32b0c5b2`) costs one extra full walk that writes nothing; it does not stop
convergence. A code reading names a candidate: `project()` reads the frontier in one read guard
(`crates/holon-loro/src/loro_sync_controller.rs:1085`) and drains the fact queue in another
(`read_incremental` :1557, `read_full` :1591). Not measured; another lane owns it.

## Missing piece

The test applied a 2001-block bulk seed through the per-transition settle, whose 30 s budget is
sized for one interaction, not for an import.

## Remedy

FIXED in the test. The harness has an entry point that applies a transition with a caller-given
settle budget (`ComposedSut::<WideE2E>::apply_settling_within`,
`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs`). The dense_patch session seeds
through it with `seed_settle_budget` = max(`CONVERGE_BUDGET`, 40 ms per block)
(`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`), 80 s for 2001 blocks, below
the 120 s wedge bound. Every other settle keeps `CONVERGE_BUDGET`.

Not done: bulk ingest of a 2001-block page costs 31-47 s in the test profile under load. It needs
a release measurement before it can be judged as a performance defect.
