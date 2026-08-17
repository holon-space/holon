---
id: 2026-07-21-rhai-float-comparisons-relative-epsilon-tolerant
date: 2026-07-21
gap: ORACLE
secondary: COVERAGE
status: UNCLASSIFIED
summary: >-
  Rhai float comparisons are relative-epsilon tolerant while the subset
  evaluator (and SQLite lowering) are strict IEEE — dual-eval PBT
  subset_eval_equals_rhai sat RED-WITH-COMMITTED-SEEDS, untriaged, since the
  generator gained mixed int/float (stale module header masked it); latent
  prod inconsistency: derived field via SQL (strict) vs Rhai Script fallback
  (epsilon) can disagree at ULP boundaries
source_line: 1068
---

## Bug

Rhai float comparisons are relative-epsilon tolerant while the subset
evaluator (and SQLite lowering) are strict IEEE — dual-eval PBT
subset_eval_equals_rhai sat RED-WITH-COMMITTED-SEEDS, untriaged, since the
generator gained mixed int/float (stale module header masked it); latent
prod inconsistency: derived field via SQL (strict) vs Rhai Script fallback
(epsilon) can disagree at ULP boundaries

## Missing piece

funnel gap: red-with-seeds tests must be triaged, not accumulated; prod fork
= register strict f64 comparisons in bounded_engine() (rank_tasks +
block-profile blast radius — ruling queued)

## Remedy

TEST FIXED+WOVEN 2026-07-21 (cycle 4; Cmp operands exact-int, directed ULP
pin, 51200 cases green); PROD FORK OPEN
