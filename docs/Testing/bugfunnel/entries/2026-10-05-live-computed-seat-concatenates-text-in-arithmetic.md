---
id: 2026-10-05-live-computed-seat-concatenates-text-in-arithmetic
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The live read seat evaluated a typed arithmetic computed field with Rhai, so
  `xi + 1` over a TEXT cell "abc" in an INTEGER column gave "abc1" with no
  condition, where `Computation::eval` and the planted SQL column refuse it.
---

## Bug
Found by a code audit of the derived-field seats (lane d45-arith, rulings
D45.a / D57.b / D62.a, ADR 0024 P1 "one semantics"). A verifier first reported
a "silent NULL" for a non-numeric operand; that NULL came only from the
test-only `TypeDefinition::enrich`, which is deleted. The production live seat
(`resolve_computed_fields_with_scope`, used by the enrich seat in
`ui_watcher::enrich_row` and the render seat `EntityProfile::build_scope`) had
a different defect: it ran every computed field on Rhai, whatever its typed
declaration said.

- A TEXT value in an INTEGER column (SQLite affinity keeps "abc" as TEXT) gave
  `"abc" + 1 = "abc1"`, recorded as success. `Computation::eval` refuses it
  with `NotNumeric`, and the planted column refuses the write (`holon_add`).
- A NULL operand gave a disclosed Rhai error `Function not found: + ((), i64)`
  where eval and SQL give NULL (D62.a).
- A live-tier declaration of arithmetic over a BOOLEAN or TEXT column was
  accepted; the persisted tier refused it.

Reachable only through a user-declared type: no shipped computed field does
arithmetic.

## Root cause
Two evaluators for one declaration. `ComputedSpec` carries a typed
`Computation`, but the read path kept only the Rhai `CompiledExpr`
(`CompiledComputedField` was `(String, CompiledExpr)`), so
`crates/holon-api/src/computed.rs` `resolve_computed_fields_with_scope` always
called `engine.eval_ast_with_scope`. Rhai's `+` concatenates a string with an
integer. The operand-kind check ran only for the persisted tier and only at the
root node (`result_kind`).

Evidence: `lane-logs/r4/red-keep.log` (22 run, 4 failed: live seat
`(Ok(()), String("abc1"))` against eval `Err(NotNumeric ...)`; live
`Function not found` against eval NULL; live declaration of `flag + 1`
accepted).

## Missing piece
The keystone's `DeclareTypedSchema` draws TEXT columns and one Concat computed
field only (`transitions/declare_typed_schema.rs`), so it cannot generate an
arithmetic derived field over a numeric column (COVERAGE). No oracle compared
the live seat with `Computation::eval` (ORACLE).

## Remedy
- `CompiledComputedField` carries the typed `Computation`; the live seat
  evaluates it with `Computation::eval`, and only `Computation::Script` runs on
  Rhai (`crates/holon-api/src/computed.rs`, `entity_profile.rs`).
- `Computation::check_arith_operands` refuses a TEXT or BOOLEAN operand under
  any arithmetic node, for every tier (`computation.rs`, `entity.rs`
  `ComputedSpec::parse`).
- `crates/holon-turso/tests/derived_field_eval_vs_sql.rs` gains a live leg:
  for every generated computation the live seat must give eval's value or
  eval's refusal, and a live declaration must be refused where the persisted
  one is. Teeth: routing the typed branch back to Rhai turns 2 tests red;
  skipping the check for the live tier turns 3 tests red.
- Open: the keystone still cannot draw an arithmetic derived field (numeric
  column kinds + an Arith declaration in `DeclareTypedSchema`).
