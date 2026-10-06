---
id: 2026-10-06-optional-id-param-refused-without-its-name
date: 2026-10-06
gap: COVERAGE
status: FIXED
summary: >-
  An optional id-bearing operation param (instantiate_template's
  replace_block) was parsed by no entity-reference seam, so a malformed value
  was refused as Invalid URI without naming the parameter.
---

## Bug
Found by a verifier (lane D69.a round 8, `lane-logs/d69a-verify8.md`, defect
V2) by code reading. `instantiate_template` with `replace_block = "a b"`
failed with `Invalid URI "a b"`; red log `lane-logs/r9-red-v2.log`.

## Root cause
`OperationDescriptor` declared only `required_params`, and
`entity_reference_params` read only those. The engine's early parse also read
only dispatcher-registered descriptors, so the engine-synthetic
`instantiate_template` was parsed by neither seam.

## Missing piece
No test sent a malformed value in an optional id position (COVERAGE).

## Remedy
`OperationDescriptor.optional_params`; `entity_reference_params` reads both
lists; the `operations_trait` macro emits `Option<T>` params into it; the
engine's early parse includes the engine-synthetic descriptors. Tests
`a_malformed_optional_id_param_is_refused_naming_the_parameter` and
`an_optional_param_declared_an_entity_id_is_a_reference`; green log
`lane-logs/r9-green-v2.log`.
