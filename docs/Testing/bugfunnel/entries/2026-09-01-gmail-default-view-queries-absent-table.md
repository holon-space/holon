---
id: 2026-09-01-gmail-default-view-queries-absent-table
date: 2026-09-01
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  D53.c ships an authored default view for every bundled integration, but
  Gmail's view queries gmail_thread, a table that is never created when the
  provider is unavailable, so the row the user can click opens a broken view.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`) while
exercising D53.c ("every bundled integration opens an authored default view")
across all five bundled providers.

All five authored views exist and their ids match the `default_view` key of
their sidecar, which is the substance of D53.c and is correct:

| provider | `default_view` | block present |
|---|---|---|
| claude-history | `claude-history-view` | yes |
| gcal | `gcal-view` | yes |
| gmail | `gmail-view` | yes |
| jsonplaceholder | `jsonplaceholder-view` | yes |
| todoist | `todoist-view` | yes |

But the backing tables do not all exist. `block:gmail-view::render::0` runs:

```sql
SELECT id, snippet, history_id FROM gmail_thread ORDER BY history_id DESC LIMIT 25
```

and that table is absent from the live database:

```
Parse error: no such table: gmail_thread
```

Gmail is enabled and therefore has a clickable sidebar row, but it is
`Unavailable` (unconfigured), so its entity tables were never created. The row
stays clickable and its authored view's query hard-fails against the schema.

The other four views' tables exist (`cc_session`, `gcal_event`, `jp_posts`,
`todoist_tasks`), so this is specific to the unavailable-provider path, not to
D53.c's wiring in general.

## Root cause

Entity-table DDL is created as part of bringing a provider up. A provider that
fails its availability check is skipped — `holon_app::mcp_integrations` logs
`Provider 'gmail' is not configured — skipping` and
`Integration 'gmail' unavailable (not configured)` — so no `gmail_*` tables are
created. D53.c's sidebar row and authored default view are published from the
sidecar's presentation metadata, which is compiled into the binary and therefore
present whether or not the provider ever came up. The two halves disagree: the
view is always reachable, its tables are conditional.

Evidence: `/tmp/dogfood2-0901/logs/app3.log`; `block` and `gmail_thread`
queried live over MCP on port 8720.

## Missing piece

No invariant asserts that **every authored default view's query resolves
against the live schema**. The state is entirely reachable — one unconfigured
bundled provider is arguably the *default* first-run state — and it is
observable in SQL, so a case that reached it would still have gone green. That
makes this an ORACLE gap rather than a coverage one. The ENVIRONMENT secondary
is that the keystone wires no MCP-client integrations at all, so neither the
views nor their tables exist in the test environment to be checked.

Related but distinct: the view's *render* behaviour when its query fails could
not be observed in this session, because the app stopped painting on a locked
display (see `2026-09-01-integration-rows-invisible-to-describe-ui`). Whether
the user sees a loud error or a silent blank panel is **unverified**.

## Remedy

Open. Proposed:

1. Add an invariant sweeping every authored default view and asserting its query
   compiles against the live schema — red today for `gmail-view`.
2. Decide the product behaviour for an unavailable provider: either create the
   entity tables unconditionally so the view renders empty, or have the view
   render a disclosed "provider not configured" state instead of executing a
   query that cannot resolve. Per the repo's error-handling philosophy a visible,
   disclosed degraded state is required — a silent blank panel is not acceptable.
3. Confirm what the failing view actually paints, once a windowed environment
   with a live display is available.
