---
id: 2026-08-06-issues-210-sql-reads-against-budget
date: 2026-08-06
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `InstantiateTemplate` issues 210 SQL reads against a 43 budget (5x): the
  main-panel projection query runs 169 times in ONE transition, 146 of them
  with IDENTICAL bindings.
source_line: 1159
---

## Bug

(budget-lane armed-gate investigation) **`InstantiateTemplate` issues 210
SQL reads against a 43 budget (5x): the main-panel projection query runs 169
times in ONE transition, 146 of them with IDENTICAL bindings.** The same
consumer re-asks the same question 146x; only 7 of 216 duplicate-SQL entries
across two runs were legitimate per-consumer fan-out. Query text: `SELECT
b.id, b.parent_id, b.depth, b.sort_key, b.content, …` — the body of the
`block` matview, built by `block_matview_select`
(`crates/holon-turso/src/schema_modules.rs:461`, installed at :550). The
consumer that re-issues it has NOT been attributed to a call site;
`crates/holon-app/src/turso_seams.rs:144` (`load_all_blocks_with_hydration`)
reads the same shape but is not established as the repeating caller. Same
mechanism, smaller magnitude, on `BlockToPage` (largest duplicate entry 18x
over 4 bindings, x7 identical) and `CreateDocument`. Max read count observed
is 220, not 210.

## Missing piece

The `inv-sql-budget` gate that measures exactly this has NEVER been armed —
`HOLON_PERF_BUDGET` was set by no justfile recipe, so only the two
`Severity::Pinned` transitions gated and every other budget was a logged
note. Secondary ORACLE: the N+1 report labelled this `"7 distinct bindings —
fan-out"`, which reads as legitimate and is why prior lanes modelled the
repeat factor as real cost instead of a defect; `distinct_bindings` alone
cannot separate "N consumers once each" from "one consumer N times".

## Remedy

OPEN → task #15. Diagnosis instrument landed 2026-08-06:
`DuplicateSql::max_repeat_per_binding`
(`crates/holon-integration-tests/src/test_tracing.rs:721`) + a
`[LEGITIMATE]`/`[REDUNDANT xN/binding]` verdict on every N+1 line, which is
what made the 146x measurable. Budgets deliberately NOT re-derived to absorb
the fan — that would encode the defect as contract; they re-derive from
LEGITIMATE counts once the redundancy is fixed, at which point the budgets
go DOWN. Arming stays opt-in (`justfile` `pbt` + `hand-authored`,
`${HOLON_PERF_BUDGET-0}`) until then.
