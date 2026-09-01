---
id: 2026-09-01-integration-rows-invisible-to-describe-ui
date: 2026-09-01
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The D53.c Integrations sidebar rows expose no entity ids and no geometry to
  describe_ui, so the new feature cannot be asserted on or driven by id from any
  windowed test or agent — only by raw pixel coordinates.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`) while trying
to exercise D53.c ("every bundled integration opens an authored default view").

`describe_ui block:default-left-sidebar` renders the Integrations section as:

```
row
  icon "sync"
  text "Integrations" (bold)
live_query
  UNEVALUATED[live_query_rows]: rows not evaluated: the node carries no
  query/query_lang/render_expr, so it cannot describe its own result
```

and its `geometry` block lists only the two `block:`-scoped tree items. The five
integration rows — which the app *is* painting, and which the element count
confirms (68 elements before enabling integrations, 106 after) — contribute
neither a described subtree nor a single geometry entry.

The practical effect: there is no `entity_id` to pass to `click`, and no
measured bounds to verify against. The only way to drive the headline feature of
this release is to compute pixel coordinates from a screenshot, which is exactly
the brittle path the MCP driving surface exists to replace. It also means no
windowed PBT can assert "the Integrations section lists the enabled providers
with the right names, icons and status symbols" through the standard
introspection surface.

Per the `holon-feature` contract a new feature ships with a covering PBT. For
the parts of D53.c that live in this sidebar section, the introspection surface
cannot currently express the assertion.

## Root cause

Two compounding causes:

1. `live_query` nodes whose rows are produced by the frontend's own evaluation
   are described as `UNEVALUATED` — the described node carries no
   `query`/`query_lang`/`render_expr`, so `describe_ui` has nothing to expand.
   The rows exist in the painted tree but not in the described tree.
2. The rows' identity is `integration:<provider>`, not `block:<id>`. The
   geometry recorder appears to index painted elements by block-scoped entity
   uri, so non-block entities record no bounds even when painted.

The sidebar's authored template does emit both an icon and an id per row —
`icon(col("icon"))`, `integration_open_default_view(#{id: col("id")})` against
`SELECT id, provider_name, display_name, icon, status FROM integration_state` —
so the data needed to describe them is present; it just never reaches
`describe_ui`.

Evidence: `describe_ui` output for `block:default-left-sidebar` and
`block:root-layout`; `integration_state` queried live over MCP on port 8720.

## Missing piece

A described-tree representation for rows produced by a frontend-evaluated
`live_query`, and geometry recording for non-`block:` entity uris. Without them
the dogfood channel — the designated final quality gate — is blind to an entire
newly-shipped surface, and the windowed PBTs cannot pin it.

This is the perception gap in its literal sense: the harness cannot see the
thing, so no assertion can be written about it.

## Remedy

Open. Proposed:

1. Describe `live_query` rows that the frontend has already evaluated, rather
   than reporting `UNEVALUATED` for a node whose rows are materialised.
2. Record geometry for painted elements keyed by any entity uri, not only
   `block:`-scoped ones.
3. Once (1) and (2) land, add the windowed assertion D53.c should have shipped
   with: the Integrations section lists exactly the enabled providers, each with
   its sidecar display name, its icon, and a status symbol at a consistent x.

## Related

Ruled out as a cause during this investigation, and worth recording as a
constraint on future dogfood passes: the session's host had a locked/sleeping
display, so GPUI stopped producing frames after boot. The MCP `screenshot` tool
then returns the last painted frame, byte-identical across real navigations, and
coordinate `click` stops hit-testing. Any dogfood pass that needs windowed
interaction requires an awake, unlocked display; model-level checks over MCP and
SQL remain valid.
