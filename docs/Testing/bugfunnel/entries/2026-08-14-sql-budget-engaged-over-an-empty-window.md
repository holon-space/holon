---
id: 2026-08-14-sql-budget-engaged-over-an-empty-window
date: 2026-08-14
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `inv-sql-budget` reported `30/30` engaged on a Loro-sync-only run whose budget
  windows observed zero SQL statements — a clean verdict over nothing was
  indistinguishable in the engagement ledger from a real comparison.
---

## Bug
A `PROPTEST_CASES=1` keystone run drew a Loro-sync-only sequence in which every
per-transition line reported `reads=0 (dedup 0)/… writes=0/… ddl=0/…`, and the
`[engagement summary]` still reported `inv-sql-budget=30/30` engaged.

Found by READING that log during the OTel research pass
(`lane-logs/research-otel-perf.md` P1) — no test verdict named it. Task-#17
instrumentation lane.

## Root cause
`InvComposedBudget::check`
(`crates/holon-integration-tests/src/pbt/composed/span_metrics.rs`) decided on
`(report.enforce, report.errors.is_empty())` alone and mapped the whole
`(_, true)` arm to `InvariantResult::Ok`. Every SQL ceiling is trivially
satisfied by an empty window, so "no violations" meant the same thing whether
the window compared 40 reads against a formula or observed no SQL at all. The
harness ledger (`composed/harness.rs:1157-1165`) counts every non-`Skipped`
verdict as engaged, so the vacuous window was recorded as a real exercise — as
was the pre-first-transition tick, where `last_transition` is `None` and no
transition has run.

## Missing piece
The verdict enum already carried the distinction — `Skipped` means "proved
nothing" — and the budget body never used it for an empty window. Nothing in the
report expressed the denominator that separates "checked and cheap" from
"checked nothing".

Taken in the triage order: COVERAGE — no, a SQL-less wiring is an ordinary draw
of the keystone alphabet. ENVIRONMENT — no, the reporting code runs in the
keystone's own wiring and its output was collected. PERCEPTION — no, the
artifact is a stderr line, fully assertable headlessly. Same shape as the F1
un-blinding this ledger records: a body returning a fake `Ok` over a subject it
never examined.

The existing vacuity guard could not close it: `ENGAGEMENT_FLOOR` only flags
invariants that are ALWAYS `Skipped`, and this vacuity presents as `Ok` —
precisely the verdict that satisfies the floor.

## Remedy
Fixed. `SqlBudgetReport` carries `observed_statements` (reads + writes + DDL of
the frozen window, fed from `TransitionMetrics` at the freeze point the budget
already reads), and the decision is extracted into the pure `budget_verdict`,
which returns `Skipped("vacuous — the budget window observed 0 SQL statements
…")` for an empty window while leaving violations outranking it. The summary now
reads `inv-sql-budget=0/N` on a SQL-less draw.

`inv-sql-budget` is deliberately NOT in `ENGAGEMENT_FLOOR`, so the honest
`Skipped` cannot false-red a legitimately SQL-less sequence. The wall/RSS
ceilings DO still run on a zero-SQL tick and the skip reason says so, rather
than claiming nothing was checked.

Pinned by `empty_window_is_skipped_not_ok`, `window_with_sql_is_ok`,
`enforcement_does_not_rescue_an_empty_window`,
`violations_outrank_the_vacuity_rule`, `unenforced_violations_stay_skipped`
(unit, over the pure verdict) and `sql_budget_is_skipped_on_an_empty_window` /
`sql_budget_passes_when_clean` (catalog-level via `run_selected`, asserting the
exact `InvariantResult` the ledger reads). Red-for-the-right-reason by deleting
the vacuity arm: `a zero-statement window must not report engagement; got Ok`
(`lane-logs/t17-RED-vacuity-arm-deleted.log`).

SCOPE CAVEAT, deliberately not overreached: this fixes ONE invariant. "Returns
`Ok` over an empty subject" is a CLASS — the remaining catalog bodies were not
audited, and the structural remedy (an engagement ledger recording a
per-invariant subject cardinality rather than a Skipped/non-Skipped bit) is left
as the proposed design, not half-built.
