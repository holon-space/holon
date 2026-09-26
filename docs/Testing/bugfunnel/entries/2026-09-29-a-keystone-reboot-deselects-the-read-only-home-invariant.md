---
id: 2026-09-29-a-keystone-reboot-deselects-the-read-only-home-invariant
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  After any keystone `Reboot`, `inv-read-only-home-refuses-writes` is silently deselected for the rest of the sequence.
---

## Bug
Found by the D229 round-6 verifier (lane d229-move) by reading and by an inverted probe. Report: `lane-logs/d229r6v-verify.md`, Defect C.

## Root cause
`boot_and_seed_wide` inserted three frontend read caps (`SutReadOnlyHomes`, `SutConditions`, `SutEditorSaves`); `reboot_wide` re-inserted only two (`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs`).

## Missing piece
Nothing asserted that a reboot keeps the invariant selection.

## Remedy
Fixed in round 7: both boots insert the caps through `insert_frontend_read_caps`, and the harness fails a run whose `Reboot` deselects an invariant (R11.4).
