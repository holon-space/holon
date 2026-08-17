---
id: 2026-07-20-duplicate-block-registration-ops-registered-twice
date: 2026-07-20
gap: PERCEPTION
secondary: COVERAGE
status: PARTIAL
summary: >-
  Duplicate block-op registration — 12 ops registered twice → duplicate
  "Delete" (and 11 others) in slash command menu
source_line: 1048
---

## Bug

Duplicate block-op registration — 12 ops registered twice → duplicate
"Delete" (and 11 others) in slash command menu

## Missing piece

registry uniqueness invariant / correspondence lock doesn't catch
double-registration

## Remedy

PARTIAL-FIXED+WOVEN 2026-07-21 — W7 OperationSubset narrows the second
provider to its 4 unique link/page ops (CRUD double-registration gone) +
fail-loud dispatcher uniqueness invariant (should_panic-proven). RESIDUAL:
structural ops double-advertised via SqlBlockOperations+LoroBlockOperations
still reach the menu (dogfood B1: Delete x2, Cycle Task State x2) —
tolerated via named STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST, tracked in the
2026-07-21 structural-op row
