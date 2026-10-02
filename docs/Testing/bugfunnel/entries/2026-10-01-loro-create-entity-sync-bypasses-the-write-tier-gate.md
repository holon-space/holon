---
id: 2026-10-01-loro-create-entity-sync-bypasses-the-write-tier-gate
date: 2026-10-01
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The Loro-leg `create_entity_sync` never asks the write tier, so a create under
  a read-only-homed page lands in the store and can never reach the .cook file.
---

## Bug
Found by the RCA of three registered keystone reds (`red-rca-report.md` in the
session scratchpad, row 1, probe `r1-loro`). Measured there: NavigateFocus to the
recipe page then CreateBlockUnderFocus on the Loro+Turso wiring was red 3/3 with
`inv-blocks-match-ref/org: 8 blocks, reference: 9`. Fix lane: i1-tier-guard.
Different from `2026-09-16-keystone-create-under-a-read-only-focus-root-panics`,
which is the SqlOnly leg where the product refuses correctly.

## Root cause
`create_entity_sync` (`crates/holon-loro/src/block_cell_registry.rs:498`)
creates the node directly. `write_tier_refusal` (`:188`) has one caller, the
text-cell open path (`:434`). The gate lives in the dispatcher
(`enforce_write_tier`, `crates/holon/src/api/operation_dispatcher.rs:1237`), and
this path does not go through it. The TUI installs the cell registry, so the
bypass is live there; GPUI does not install it (`frontends/gpui/src/di.rs`),
so it is latent. A refusal error would reach `edit_target_id`, which `.expect`s
(`crates/holon-frontend/src/view_event_handler.rs:131-143`).

## Missing piece
The keystone draws it but classifies it as the known red `org-blocks-ref-diverge`
(wrong attribution), and `inv-read-only-home-refuses-writes` compares only
declared blocks, so a new child under a read-only page passes.

## Remedy
`create_entity_sync` and the async `create_entity` both call `refuse_create_under`
(`crates/holon-loro/src/block_cell_registry.rs`) and return a typed `EditRefused`
under a read-only parent; the registry takes the write-tier authority at
construction, so it cannot run without one (lanes I1 and I1b). A read-only page
offers no creation slot, so `edit_target_id` no longer meets a refusal. Test
`create_entity_sync_under_a_read_only_parent_is_refused_typed`: red before the fix
(i1-tier-guard `lane-logs/i1-red.log`), green after (`lane-logs/i1r2c-green-unit.log`),
red with the refusal removed in place (`lane-logs/i1r2b-teeth-gate.log`,
`lane-logs/i1-teeth-both.log`, restore sha-checked); async leg: i1b-fail-closed
`lane-logs/i1b-f3-red.log`, `-f3-green.log`, `-f3-teeth.log`.
