---
id: 2026-10-06-create-params-ids-unparsed-at-boundary
date: 2026-10-06
gap: COVERAGE
status: FIXED
summary: >-
  A block create with a malformed parent_id panicked, a malformed
  after_block_id was accepted, and a malformed id was refused without naming
  the parameter, because no create descriptor declared those ids.
---

## Bug
Found by a verifier (lane D69.a round 9, `lane-logs/d69a-verify9.md`, O2) by
code reading. Through `BackendEngine::execute_operation`, today's behavior
for `create` with the value `"a b"` was: `parent_id` panicked in
`EntityUri::new`, `after_block_id` returned Ok, `id` was refused without
`parameter 'id'` in the message. Red log `lane-logs/r10-red.log`.

## Root cause
The only create descriptors declared one `fields` map (the `CrudOperations`
macro) or no param (the SQL provider), so `entity_reference_params` saw no id.

## Missing piece
No test sent a malformed id inside create's param bag (COVERAGE).

## Remedy
The SQL provider's create descriptor declares `id`, `parent_id` and
`after_block_id` in `optional_params`. Test
`a_malformed_id_in_create_params_is_refused_by_name`; green log
`lane-logs/r10-green.log`; teeth `lane-logs/r10-teeth.log`.
