---
id: 2026-10-05-a-numeric-write-to-a-text-column-lands-as-a-number-string
date: 2026-10-05
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A number written through the generic authoring door or MCP into a TEXT column
  (shopping `count`) is stored by SQLite affinity as "2" or "2.0", and that
  value then differs from the peer's text on every sync poll.
---

## Bug
Found by the verifier of the shopping free-text-amount lane (round 2, defect D5),
by code audit. Not reproduced in a running app.

## Root cause
No write path checks a value against the field's declared `sql_type`. The shopping
type declares `count` as TEXT, so a `Value::Integer(2)` or `Value::Float(2.0)` from a
caller lands as the text "2" or "2.0". `same_value`
(crates/holon-connections/src/reconcile.rs) has no String↔number arm, so the stored
value never equals the peer's text, and every poll emits a `SetColumn` for it.
The fresh-DB no-churn property holds only while every local writer supplies text.

## Missing piece
A type-checked write: nothing parses an incoming value against the declared field
type at the operation boundary. No test writes a number into a TEXT column.

## Remedy
Open. To be closed by the string-lens `format:` field work, which gives a field a
declared value format that a write is parsed against.
