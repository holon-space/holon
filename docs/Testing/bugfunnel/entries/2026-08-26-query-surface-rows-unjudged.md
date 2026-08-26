---
id: 2026-08-26-query-surface-rows-unjudged
date: 2026-08-26
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A row rendered inside a nested query surface that is stale in the reactive
  registry itself, or that comes from an inline live_query node, is judged by
  no invariant — inv-main-panel-rows-match-focus deliberately excuses it and
  nothing else covers those rows.
---

## Bug

No user-visible defect yet: this is a **coverage hole created by a deliberate
oracle change**, found by adversarial verification of the D36.a lane
(`inv-qrows`), recorded per the CLAUDE.md rule that a gap discovered outside an
automated test is triaged like any other escape.

D36.a ruled that a `live_query` rendered in a page body may legitimately
surface rows from OUTSIDE the focused subtree, so
`inv-main-panel-rows-match-focus` had to stop calling those rows stale. The
first cut exempted the query surface's ENTIRE rendered subtree. The verifier
built the laundering witness:

```
live_block[host]                     (host owns a query source)
    > tree_item[legit-result]
    > tree_item[stale-leftover]      <- the query never produced this
JUDGED IDS = {default-main-panel, doc, host}
```

and searched for a compensating oracle. There is none — see **Missing piece**.

## Root cause

The exemption was id-blind: `outline_content_ids`
(`crates/holon-integration-tests/src/pbt/invariants/bodies/main_panel_rows_match_focus.rs`)
returned at the boundary without ever consulting the query's result set.

That half is now FIXED (see Remedy). What remains is the part no available
oracle can reach:

1. **Registry-stale rows.** The exemption now asks
   `SutRenderer::collection_row_ids` — what the surface's collection was
   actually delivered. If a matview delete never propagates, the stale row sits
   in the ROW SET itself; render and row set agree and are both wrong, so the
   comparison passes. Catching this needs the REFERENCE to predict the query's
   result set, i.e. a second SQL engine in the model — the hand-mirror this
   project deliberately does not build.
2. **Inline `live_query(…)` rows.** Such a node carries no `entity_id`; its
   registry entry is a synthetic `query:<hash>` key that is not addressable
   from the widget tree. Its subtree is still blanket-exempt. The shipped
   main-panel layout has one — the "Linked references" backlinks accordion
   (`assets/default/index.org:22`).

Measured this session: a query surface renders ONLY the rows its query returned
— a result row's outline children do not render there, because `tree(...)` is
built from the row set, not the block tree. So the exposure is exactly the two
cases above, not a general "everything under a query block is invisible".

Method, since the artifact is indirect: a child (`gamma offspring`) was added
under the result row in a SCRATCH copy of the cross-document parity scenario,
with `And the widget does not contain "gamma offspring"` appended, and the
corpus was replayed. `run_feature_strict` hard-panics on an unmet `Then`, so a
green corpus means the assertion held. The scratch fixture was then restored
byte-identical, so neither the assertion nor its text survives in the tree.
`target/lane-logs/measure-surface-children.log` does NOT contain the assertion
text — the gherkin harness does not echo step text. What it does carry is the
line `replaying "A query surfaces a row from outside the focused subtree"
(6 steps)` against the shipped scenario's 5, plus `test result: ok`: the sixth
step was the added assertion and the run was green. Anyone re-checking this
should redo the scratch edit rather than grep the log for `gamma`.

## Missing piece

No invariant owns "rows rendered under a nested query surface ⊆ that query's
predicted result set". Verified by search this session; every candidate fails
for a specific reason:

- `inv-viewmodel-entity-ids-subset-of-data` — computes
  `missing = tree_ids − data_ids − ref_known`. A stale row is by definition a
  REAL ref-known block, so it is subtracted away. Catches phantom ids only.
- `inv-watch-rows-match-ref` — judges MCP-actor watches
  (`ReferenceState.mcp.active_watches`, `ref_caps/watches.rs:25-46`). A
  block-level `holon_sql` source rendering into its own `live_block` registers
  no such watch, and the body never reads the rendered tree.
- `inv-viewmodel-decompiled-rows-match-query` — SUT-internal (rendered vs the
  SUT's OWN query rows, so a stale matview row appears on both sides), scoped
  to the ROOT render expr, and deselected in the keystone (0/20).

## Remedy

PARTIAL, and the partiality is the point of this entry.

Fixed: the exemption is now result-set-aware. A row rendered under a query
surface is excused only if `collection_row_ids` says that surface's collection
currently holds it; an UNWATCHED surface excuses nothing. Pinned by
`a_row_the_surface_never_delivered_stays_judged` and
`an_unwatched_surface_excuses_nothing`, both proven red under the blanket
exemption (`target/lane-logs/countercase-blanket-red.log`), and by an
end-to-end inversion showing the registry read is load-bearing rather than
decorative (`target/lane-logs/registry-read-is-loadbearing-red.log`: forcing
the registry answer empty reds the parity scenario with
`stale ids: ["block:beta-needle"]`).

OPEN, needing a decision rather than an implementation:

- **Registry-stale rows.** Requires a ref-side prediction of the result set.
  Recommend NOT building it (a second SQL engine in the model); the alternative
  is a differential check against a recomputed query, which is
  `inv-matview-consistent-with-recompute`'s territory extended to per-block
  query surfaces. Worth its own ruling.
- **Inline `live_query` rows.** Addressable if the widget snapshot carried the
  synthetic watcher key. `ViewKind::LiveQuery` already surfaces `query`,
  `query_lang` and `query_context_id` into `props`; adding the registry key
  would close this half without a model-side query engine. Smaller and more
  tractable than the first item.

Keystone repro: the interaction IS generatable (the composed keystone renders
query surfaces; `inv-main-panel-rows-match-focus` is engaged 20/20 under
`HOLON_PBT_FORCE_FULL=1`). This is an ORACLE gap, not a COVERAGE one — the
states are reached, nothing judges them.
