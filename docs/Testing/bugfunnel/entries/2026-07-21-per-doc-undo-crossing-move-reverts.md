---
id: 2026-07-21-per-doc-undo-crossing-move-reverts
date: 2026-07-21
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Per-doc undo of a crossing move reverts a concurrent peer's UNRELATED
  reparent on merge — CRDT stays convergent but a concurrent structural edit
  is silently lost (loro fork 1.13.7 inherits; verified on 1.11.1). Impacts
  ADR 0028 §7 inverse-crossing undo design.
source_line: 1062
---

## Bug

Per-doc undo of a crossing move reverts a concurrent peer's UNRELATED
reparent on merge — CRDT stays convergent but a concurrent structural edit
is silently lost (loro fork 1.13.7 inherits; verified on 1.11.1). Impacts
ADR 0028 §7 inverse-crossing undo design.

## Missing piece

no peer-move alphabet / directed inverse-crossing oracle in the keystone
(Inc-W5.1 specified)

## Remedy

OPEN (characterized, canary-pinned:
move_storm_convergence::undo_across_crossing 5/5; W5)
