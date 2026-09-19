---
id: 2026-09-19-describe-ui-cannot-see-live-query-rows-and-misstates-why
date: 2026-09-19
gap: COVERAGE
secondary: PERCEPTION
status: OPEN
summary: >-
  `describe_ui` returns `unevaluated` for a `live_query` node and gives as its
  reason that the node carries no query, while printing that node's `query` and
  `query_lang` two lines below — so the dogfood channel's rendered-vs-internal
  cross-check is blind to every live-query list and is told a false reason.
---

## Bug

Found by the `dogfood-integ` lane. The sidebar's Integrations section is a
`live_query` node. `describe_ui {"block_id":"block:default-left-sidebar",
"format":"json"}` returns 1.79 MB of tree in which no integration row appears at
all, and the node itself comes back as:

```json
{
  "content": {
    "widget": "unevaluated",
    "mechanism": "live_query_rows",
    "reason": "rows not evaluated: the node carries no query/query_lang/render_expr, so it cannot describe its own result"
  },
  "query": "SELECT id, provider_name, display_name, icon, status FROM integration_state WHERE enabled = 1 ORDER BY display_name ASC",
  "query_lang": "holon_sql",
  "query_context_id": "block:006841ef-a2e7-ce1d-0332-9370bb5d6a5c"
}
```

The reason states the node carries no `query` or `query_lang`. The same object
carries both.

A screenshot of the same region
(`lane-logs/dogfood-integ-evidence/shots/03-integrations-crop.png`) shows all
six rows painted correctly and matching the mirror, so the UI is fine — the
INTROSPECTION is what fails.

## Root cause

Not isolated. The `unevaluated` branch is reached for a reason other than the
one it prints; whatever condition it actually tests is not "the node carries no
query". A wrong reason string on a diagnostic path is worse than no string,
because it sends the reader to check a field that is demonstrably populated.

## Missing piece

The dogfood-explorer protocol's central instruction is "verify BOTH surfaces
after every mutating step … divergence between the two is itself a bug"
(`.claude/skills/dogfood-explorer/SKILL.md` §2 step 5). For any list rendered by
`live_query` — the integrations section, and every query block in the vault —
that cross-check cannot be performed at all. The channel falls back to
screenshots, which is exactly the perception-only evidence the MCP surface
exists to replace.

Nothing tests that `describe_ui` can describe a `live_query` node's rows, and
nothing tests that a diagnostic reason string is consistent with the object it
is attached to.

## Remedy

Open. Two things:

1. Make `describe_ui` render a `live_query` node's delivered rows, so the
   rendered-vs-internal cross-check reaches the widget class that carries most
   of the app's data.
2. Until then, fix the reason string to name the condition actually tested. A
   diagnostic that contradicts the payload beside it costs the reader more than
   silence.
