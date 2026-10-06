---
id: 2026-10-06-malformed-block-id-on-a-structural-op-panics-at-the-dispatch-edge
date: 2026-10-06
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A malformed block id ("a b") on indent, outdent, move_block, set_field or
  cycle_task_state panics the production write route instead of returning an
  error that names the parameter and the value.
---

## Bug
Found by verifiers of lane D69.a. Round 6, probe P4: `block.indent` with
`id = "a b"` through the provider's `execute_operation` panicked at
`crates/holon-api/src/entity_uri.rs:134`; an empty id was accepted as
`block:`. Round 7 (`lane-logs/d69a-verify7.md`, defect D1): after the round-7
fix, the PRODUCTION route — `BackendEngine::execute_operation` — still
panicked on the same input.

Repro on the production route: `execute_operation(block, indent,
{id: "a b"})` on a `BackendEngine`. Executed in
`crates/holon/tests/malformed_reference_engine_route.rs`
(`a_malformed_id_through_the_engine_is_refused_by_name`): `indent`, `outdent`,
`move_block` (malformed `id`, and malformed `parent_id`), `set_field`
(`task_state`, and `content`) and `cycle_task_state` all panicked; red log
`lane-logs/r8-red-d1d2.log`.

## Root cause
Two places promoted an id from outside the system with the panicking
`EntityUri::from_raw`:
1. The `#[operations_trait]` macro dispatch edge
   (`crates/holon-macros/src/operations_trait.rs`, the `EntityUri` branch).
2. The engine's own steps that read an op's ids BEFORE the dispatcher's
   entity-reference parse runs (`crates/holon/src/api/operation_engine.rs`):
   `rehomed_root` for the re-homing ops (traced by the verifier), and other
   pre-dispatch steps for `set_field` and `cycle_task_state` (measured as
   panics, not traced to a line). Admission does not screen the id:
   `footprint_of` skips a value that is not schemed.

## Missing piece
No generator or table test feeds a malformed id to a write; the keystone mints
only well-formed ids (COVERAGE). The round-7 test then took the provider-direct
route, which skips the engine's pre-dispatch steps, so the fix was declared
done while the production route still panicked (ENVIRONMENT: the failing code
path did not run in the test's wiring).

## Remedy
1. The macro edge calls `EntityUri::try_from_raw_param(param, value)`. Test
   `unheld_write_tests::a_malformed_or_empty_id_on_a_structural_op_is_refused_by_name`
   (`crates/holon-loro/src/loro_block_operations.rs`); red log
   `lane-logs/r7-red-item1.log`.
2. `execute_admitted` runs the dispatcher's entity-reference parse
   (`OperationDispatcher::parse_entity_references_of`) before any step that
   reads the op's ids, so the refusal is the dispatcher's
   `UnschemedEntityReference`, which names the parameter and the value. Test
   `a_malformed_id_through_the_engine_is_refused_by_name` drives the
   `BackendEngine` route; green log `lane-logs/r8-green-d1d2.log`.

`instantiate_template` is an engine-synthetic op that the dispatcher's
providers do not declare, so the early parse does not see its params. A
malformed `template_id` there is refused with "Invalid URI", which names the
value but not the parameter. It does not panic.
