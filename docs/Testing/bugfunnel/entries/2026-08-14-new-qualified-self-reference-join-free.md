---
id: 2026-08-14-new-qualified-self-reference-join-free
date: 2026-08-14
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  NEW-B — a qualified self-reference in a join-free recursive arm panics the
  process.
source_line: 712
---

## Bug

(task-#12 turso-triage lane; found by the B-audit at pin `54f3cc5`) **NEW-B
— a qualified self-reference in a join-free recursive arm panics the
process.** `SELECT t.n+1, t.p\ | \ | '-'\ | \ | CAST(t.n+1 AS TEXT) FROM t`
aborts with `unreachable code: Qualified should be resolved to a Column
before translation` (`translator.rs:2865`), exit 101 — a hard abort, not a
returned `Err`, so fail-loud error handling cannot see it. Repro
`lane-logs/baudit-probes/02_qual_cte_alone.sql`.

## Root cause

task-#12 turso-triage lane, found by the B-audit
(`lane-logs/research-j1p-unpark.md`) driving `tursodb` at our pin `54f3cc5`
— no test produced it: **NEW-B, a QUALIFIED self-reference in a join-free
recursive arm PANICS THE PROCESS.** `... UNION ALL SELECT t.n+1,
t.p||'-'||CAST(t.n+1 AS TEXT) FROM t WHERE t.n<4` aborts with `panicked at
core/translate/expr/translator.rs:2865: internal error: entered unreachable
code: Qualified should be resolved to a Column before translation`, exit
101. This is a hard abort, not a returned `Err` — inside a Holon process it
takes the process down rather than surfacing an error, so our fail-loud
error handling cannot even see it. Adding a join fixes it
(`04_qual_with_join.sql`). Repro
`lane-logs/baudit-probes/02_qual_cte_alone.sql`. Same COVERAGE gap and same
closed rung as NEW-A above (fuzzer could not emit top-level `WITH
RECURSIVE`; closed by D5.b, task #10); secondary ENVIRONMENT. NOT FIXED IN
HOLON, no production exposure, same architecture test now guards the shape.
Does NOT reproduce at fork head `a94102c2` — resolution path is the re-pin,
task #22.)

## Missing piece

Same ungeneratable join-free shape as NEW-A: unreachable from any Holon
query, and the fuzzer could not emit top-level `WITH RECURSIVE`.

## Remedy

NOT FIXED IN HOLON (engine defect), no production exposure. Fuzzer rung
CLOSED by D5.b (task #10); same architecture test guards the shape. Does not
reproduce at fork head `a94102c2` — resolution is the re-pin, task #22.
