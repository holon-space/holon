---
id: 2026-08-19-ivm-two-json-extract-predicates-matview-empty
date: 2026-08-19
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A matview whose WHERE ANDs TWO computed (json_extract) conjuncts is never
  populated (0 rows) — the turso-6f shared-`__temp_filter_expr` residual — while
  one json_extract, or json_extract AND a plain predicate, maintains correctly.
---

## Bug
While building the differential IVM PBT for the anti-join bug
(2026-08-19-ivm-antijoin-matview-silently-empty) I probed which chained-matview
shapes the fork maintains. A watch matview over the `block` matview whose WHERE
is `json_extract(properties,'$.task_state')='TODO' AND
json_extract(properties,'$.gate')='G1'` stays EMPTY after an insert, while the
same view with only the first predicate is maintained immediately. Found by
agent test-probing (lane `lane-ivm-antijoin`), not yet by Martin — but it sits
directly under the Now.org planning surface (which filters on both
`task_state` and `gate`).

## Root cause
Differential probe over the faithful prod schema (block_raw + per-junction agg
matviews + chained `block` matview), single INSERT, 300ms+ settle:

| WHERE shape | matview | fresh | verdict |
|---|---|---|---|
| `json_extract task_state` | 1 | 1 | OK |
| `json_extract task_state AND json_extract gate` | 0 | 1 | DIVERGED |
| `json_extract task_state AND id != 'zzz'` | 1 | 1 | OK |

So the trigger is TWO **computed** conjuncts ANDed — not two predicates in
general (a json_extract AND a plain column predicate is fine), and not a single
json_extract. It is NOT chaining (the earlier framing): the source being a
matview is incidental.

**This IS the turso-6f flagged residual.** Their 8-shape bisect localized the
anti-join silent-accept to the projection rewrite's catch-all aliasing an
expression onto the shared `__temp_filter_expr` temp column. The residual they
flagged — "two representable computed conjuncts sharing ONE temp column, no
subquery — the IsNull-family hazard" — is exactly this shape: two `json_extract`
conjuncts collide on the same temp column and the compiled filter goes
always-false. Reachability into a loud-refusal vs staying silently-wrong is
UNCONFIRMED; the turso verifier is probing it. Do NOT fix on the holon side
pending their verdict.

## Missing piece
ORACLE: no differential property asserted matview-served ≡ fresh recompute over
multi-computed-conjunct filters. The keystone's
`inv-matview-consistent-with-recompute` never stands up such a view.

## Remedy
- OPEN — turso-6f residual, their verdict pending. NOT fixed here.
- The anti-join render-unblock does NOT cover this shape: two `json_extract`
  conjuncts have no `Exists`/`InSubquery` node, so `sql_ivm_maintainable`
  correctly returns `true` (it is NOT a subquery-predicate) and the query is
  still served from the (silently-empty) matview. Widening the predicate to
  flag multi-computed-conjunct filters is NOT the right holon fix: the correct
  resolution is the engine making the temp-column collision refuse loudly (the
  turso fix's remit), after which — if it becomes a loud CREATE error — the
  eager path could pick it up off the DDL error. Kept OPEN, flagged to the team.
