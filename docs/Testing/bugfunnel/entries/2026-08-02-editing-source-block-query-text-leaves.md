---
id: 2026-08-02-editing-source-block-query-text-leaves
date: 2026-08-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Editing a source block's QUERY text leaves its rendered collection
  permanently broken until the app is restarted. Observed sequence on one
  block (`block:cc-conversation::src::0`): (1) after a PRQL compile error was
  fixed on disk and re-ingested — DB content confirmed updated via
  `execute_raw_sql` — the render kept showing the OLD compile error for
  minutes; (2) after re-navigating it briefly rendered correctly (4363 items);
  (3) after the next query edit it went to `list [0 items]` and stayed there
  across further edits, re-navigation and a revert to the exact text that had
  worked, while `execute_source_block` on the same block kept returning rows;
  (4) renaming the block id to get a fresh watch left it at `(loading)`
  indefinitely. No error surfaced in the UI or the log at any point.
source_line: 1142
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
Editing a source block's QUERY text leaves its rendered collection
permanently broken until the app is restarted. Observed sequence on one
block (`block:cc-conversation::src::0`): (1) after a PRQL compile error was
fixed on disk and re-ingested — DB content confirmed updated via
`execute_raw_sql` — the render kept showing the OLD compile error for
minutes; (2) after re-navigating it briefly rendered correctly (4363 items);
(3) after the next query edit it went to `list [0 items]` and stayed there
across further edits, re-navigation and a revert to the exact text that had
worked, while `execute_source_block` on the same block kept returning rows;
(4) renaming the block id to get a fresh watch left it at `(loading)`
indefinitely. No error surfaced in the UI or the log at any point.

## Missing piece

The keystone edits block CONTENT, not the query text of a source block, so
'requery after a query-text change' is not an exercised transition and the
stale/empty end state has no oracle. Missing piece = a transition that
rewrites a `holon_prql`/`holon_sql` source block and an invariant that the
rendered collection converges to the NEW query's rows.

## Remedy

OPEN — diagnosis only. Authoring a query-driven page is effectively a
restart-per-edit loop today, which is the reason this dogfood run cost
several hours.
