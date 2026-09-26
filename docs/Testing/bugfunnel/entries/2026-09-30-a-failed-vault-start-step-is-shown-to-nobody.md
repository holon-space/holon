---
id: 2026-09-30-a-failed-vault-start-step-is-shown-to-nobody
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When a step of the vault's start fails after the sync started (the file walk, the scan convergence, the fileless-page write, the undone-deletion settle, the title repair), Holon only logs it.
---

## Bug
Found by reading by the D229 round-7 verifier (Residual 1) and again by the round-8 verifier for the file walk (lane d229-move). Reports: `lane-logs/d229r7v-verify.md`, `lane-logs/d229r8v-verify.md`.

## Root cause
`crates/holon-orgmode/src/di.rs` pushed each failure to the scan failures, and `wiring.rs` reduced them to `tracing::error!`.

## Missing piece
No fault injection at the boot steps.

## Remedy
Fixed in round 8 (four steps) and round 9 (the walk, failure point `scan_vault_files`): each raises `VaultStartIncomplete { step, cause }`. Pinned by `a_failed_start_step_of_the_vault_is_disclosed` and `a_vault_walk_that_fails_at_boot_is_disclosed`.
