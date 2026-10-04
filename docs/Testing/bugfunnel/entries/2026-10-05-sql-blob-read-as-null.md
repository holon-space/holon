---
id: 2026-10-05-sql-blob-read-as-null
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A BLOB value in a SQL result row was read as Value::Null without an error, so
  bytes became indistinguishable from an absent value.
---

## Bug
Found by code audit in the D45 spike (lane d45-arith, round 1): the one SQL
row-hydration function, `turso_value_to_value`
(crates/holon-turso/src/turso.rs), mapped `turso_core::Value::Blob(_)` to
`Value::Null`. The query path and the CDC path both use it, so any row that
held a BLOB (a user query with `X'..'`, `randomblob()`, `zeroblob()`, or a
BLOB column) reported NULL for that cell and the read succeeded.

## Root cause
`Value` has no bytes kind. The conversion picked `Null` as the stand-in
instead of refusing. No Holon schema stores a BLOB today, so nothing visible
broke; the defect was latent for user-authored queries.

## Missing piece
`test_turso_value_to_value_conversions` covered NULL, INTEGER, REAL and TEXT,
not BLOB. No keystone generator emits a query or a value that produces a BLOB.

## Remedy
`turso_value_to_value` refuses a BLOB with `NotJson::Blob { path, len }`
(crates/holon-pattern/src/value.rs); the callers already name the column (and
the query, on the query path). Test:
`turso::tests::a_blob_is_refused_not_read_as_null` (red before the fix, log
lane-logs/d57-red.log of lane d45-arith).
