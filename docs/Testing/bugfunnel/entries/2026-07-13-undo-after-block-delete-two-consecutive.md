---
id: 2026-07-13-undo-after-block-delete-two-consecutive
date: 2026-07-13
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Undo after block delete: two consecutive `undo` calls return "Operation
  undone successfully" with ZERO observable effect — deleted leaf
  `block:undo-2` not restored, no other block/marks/junction changed
  (full-table diff). Stack top is polluted by the spurious identical-content
  blur commits (row above): undoing them is an invisible no-op, so "success"
  toasts eat undo presses while the delete stays unreachable — current-chain
  sibling of row 86's poison class; positive: unrelated-block destruction
  (dogfood #4 P1) did NOT reproduce
source_line: 975
---

## Bug

Undo after block delete: two consecutive `undo` calls return "Operation
undone successfully" with ZERO observable effect — deleted leaf
`block:undo-2` not restored, no other block/marks/junction changed
(full-table diff). Stack top is polluted by the spurious identical-content
blur commits (row above): undoing them is an invisible no-op, so "success"
toasts eat undo presses while the delete stays unreachable — current-chain
sibling of row 86's poison class; positive: unrelated-block destruction
(dogfood #4 P1) did NOT reproduce

## Missing piece

keystone still has no undo rungs (U5 open); no invariant "undo reports
success ⟹ projected state changed (or loudly reports no-op/irreversible)"

## Remedy

OPEN — dogfood #5; fail-loud gap: a no-op undo must say so, not claim
success
