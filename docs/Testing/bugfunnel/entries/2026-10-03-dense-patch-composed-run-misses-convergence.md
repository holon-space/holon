---
id: 2026-10-03-dense-patch-composed-run-misses-convergence
date: 2026-10-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  dense_patch a_patch_over_the_document_block_bound sometimes finds no combined projection fixed
  point within 30 s, after a steady-state full-reseed leak on the dense page write.
---

## Bug

`dense_patch_engine_exact a_patch_over_the_document_block_bound` fails in `converge_projections`
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:243`): `projections did not reach a
combined fixed point within 30s`. Seen 1/1 in the full core nextest run on main `03078ee3` (night
triage `main-reds/full-1.log:4088`) and 1/8 alone at load 11 (`main-reds/iso/B-6.log:119`). Found
by the reds triage of the night of 2026-10-03.

## Root cause

Not found. Before the timeout the run logs `[RESEED-ORACLE] FIRST steady-state full-reseed LEAK`
twice on `WriteOrgFile dense_page_0.org`, first `reason=oversized`, then
`reason=empty_pending_moved_frontier` (`main-reds/iso/B-6.log:115-116`). Candidate, not measured:
the leaked full reseed keeps the Loro->SQL projection churning past the 30 s budget.

## Missing piece

No deterministic case reaches the leak; the property reaches it only on some draws and loaded
hosts.

## Remedy

Open. Next: replay the failing case with the reseed-leak reason logged per pass and check whether
the projection still changes when the 30 s budget ends.
