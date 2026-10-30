---
id: 2026-10-30-a-refused-recipe-is-not-disclosed-while-the-boot-backlog-is-held
date: 2026-10-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  After a reboot, a vault file that its format refused showed no
  `vault-ingest-failed` disclosure until the read-only backlog read the file
  again, so a held or slow backlog hid a degraded vault.
---

## Bug
A keystone tour run (agent exploration of the hand-authored transitions)
broke an ingested recipe, rebooted with the recipe backlog held
(`RebootWithBacklogHeld`) and found no `vault-ingest-failed` condition in
effect after the boot. `inv-conditions-match-ref` went red on the
hand-authored case
`a-refused-recipe-stays-disclosed-while-the-boot-backlog-is-held`
(crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl).

## Root cause
The refusal lived only in process memory: the condition bus's refused-file map
(crates/holon-conditions/src/condition_bus.rs:1258), raised by the ingest's
error branch (crates/holon-filesystem/src/file_sync_controller.rs:3828 on the
base). A new process raised it again only when its controller read and refused
the file again. The boot (`initialize`, crates/holon-filesystem/src/file_sync_controller.rs:1879
on the base) persisted nothing about refusals and read nothing back, and D108.a
holds read-only files behind the writable ones, so between boot and the
backlog's read the vault looked healthy.

## Missing piece
No generated sequence combined a refused ingest with a reboot whose backlog
is held: `BreakIngestedRecipe` and `RebootWithBacklogHeld` existed, but no
case put them in that order.

## Remedy
The `file` row keeps the refusal (`ingest_refusal`: format, reason, and the
file's mtime and size; crates/holon-turso/sql/schema/files.sql). A refused
ingest writes it, a successful ingest clears it, and the boot discloses each
recorded refusal whose file still has that mtime and size before the backlog
runs (`raise_recorded_refusals` in
crates/holon-filesystem/src/file_sync_controller.rs). Pinned by the two
keystone hand-authored cases above and by
`a_repaired_recipe_clears_its_recorded_refusal` and
`a_refused_recipe_changed_while_the_app_is_off_is_judged_again`
(crates/holon-integration-tests/tests/cook_vault_ingest.rs).
