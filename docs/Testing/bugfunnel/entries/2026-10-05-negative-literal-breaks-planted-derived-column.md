---
id: 2026-10-05-negative-literal-breaks-planted-derived-column
date: 2026-10-05
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A derived field with a negative literal could not be planted as a matview
  column (the CREATE failed), and i64::MIN was planted as a REAL.
---

## Bug
Found in the D45 spike (lane d45-arith, round 1): the fork's matview planner
refuses unary minus at CREATE ("Cannot convert LogicalExpr to AST Expr:
UnaryExpr { op: Negative, .. }"). `value_to_sql_literal`
(crates/holon-api/src/computation.rs) inlined a negative number as `-5` or
`-1.5`, so a planted column such as `xi * -5`, `xf + -1.5`, or a `switch` with a
label `-9` failed its DDL. `i64::MIN` was inlined as `-9223372036854775808`,
which SQL reads as unary minus over a REAL, so even where it planted it was not
the integer the declaration named.

## Root cause
SQL has no negative numeric literal; `-5` is unary minus applied to `5`. The
expression parser lowers `-x` to `0 - x`, but a `switch` label and a
programmatic `Computation::Lit` keep the negative value, and the inliner
spelled it with the minus sign.

## Missing piece
The eval-vs-SQL tests (crates/holon-turso/tests/derived_field_eval_vs_sql.rs)
drew no negative literals. The keystone authors no derived fields, so it cannot
plant one.

## Remedy
`value_to_sql_literal` spells a negative number as `CAST('<n>' AS INTEGER)` or
`CAST('<f>' AS REAL)`, which the planner accepts and which is exact for
`i64::MIN`. Tests: `a_negative_literal_plants` and the proptest
`planted_arithmetic_gives_evals_value_or_evals_refusal`, whose literal pool
holds negative numbers (both red before the fix).
