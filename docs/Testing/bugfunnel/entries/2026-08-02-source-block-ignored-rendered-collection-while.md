---
id: 2026-08-02-source-block-ignored-rendered-collection-while
date: 2026-08-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A source block's `sort` and `take` are IGNORED by the rendered collection
  while `filter` is honoured. `from cc_session | filter message_count > 0 |
  sort {-modified} | take 30` returns 30 correctly-ordered rows through
  `execute_source_block`, but the same block renders 251 items in arbitrary
  (row-id) order — verified by reading the rendered date column: 07-18, 07-14,
  08-02, 07-26, 07-13, 07-23. Same on three other sections (`take 40` → 387
  items, → 1808 items). Consistent with the render path being backed by a
  matview that drops ORDER BY/LIMIT. Consequence: a page cannot bound or order
  ANY collection; the only page-layer workaround is to encode the bound as a
  `filter` (a time window), and ordering has no workaround at all.
source_line: 1141
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710) A
source block's `sort` and `take` are IGNORED by the rendered collection
while `filter` is honoured. `from cc_session | filter message_count > 0 |
sort {-modified} | take 30` returns 30 correctly-ordered rows through
`execute_source_block`, but the same block renders 251 items in arbitrary
(row-id) order — verified by reading the rendered date column: 07-18, 07-14,
08-02, 07-26, 07-13, 07-23. Same on three other sections (`take 40` → 387
items, → 1808 items). Consistent with the render path being backed by a
matview that drops ORDER BY/LIMIT. Consequence: a page cannot bound or order
ANY collection; the only page-layer workaround is to encode the bound as a
`filter` (a time window), and ordering has no workaround at all.

## Missing piece

No invariant compares a rendered collection against its own query's result
LIST (order and length), only against membership — so an ordering/limit loss
is invisible. Missing piece = the same `rendered items == query rows` oracle
proposed for the nested-live_query row, stated over the ORDERED sequence.

## Remedy

OPEN — diagnosis only. Needs a ruling on whether `take`/`sort` should be
honoured (rewriting the projection) or REFUSED loudly at compile time;
silently dropping them is the one option the error-handling philosophy
excludes.
