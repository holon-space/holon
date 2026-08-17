---
id: 2026-08-07-warm-boot-sidebar-integrations-paints-row
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  On a warm boot the sidebar Integrations `live_query` paints its row with
  empty column values
source_line: 1167
---

## Bug

(overnight dogfood-explorer, same session) **On a warm boot the sidebar
Integrations `live_query` paints its row with empty column values** — a bare
bullet, no icon text, no provider name, no timestamp — while `SELECT * FROM
sync_states` returns `provider_name='orgmode', updated_at='2026-08-07
00:40:07'`. Rendered-vs-internal divergence, and DURABLE: still blank
minutes after boot, so it is not a settling transient. The discriminator is
boot state, not the query: on the FIRST boot of the same vault the identical
row rendered correctly as "orgmode 2026-08-07 …"; only the second,
already-populated-DB boot renders it blank. The shipped SQL is `SELECT
provider_name, updated_at FROM sync_states ORDER BY provider_name ASC` with
a `row(…text(col("provider_name"))…)` template.

## Root cause

overnight dogfood — on a WARM boot the left sidebar's Integrations
`live_query` paints its row as a bare bullet with NO text, while
`sync_states` holds `provider_name='orgmode', updated_at='2026-08-07
00:40:07'`. Rendered-vs-internal divergence, and DURABLE — still blank
minutes after boot, not a settling transient. On the FIRST boot of the same
vault the identical row rendered correctly ("orgmode 2026-08-07 …"), so the
trigger is the already-populated-DB path, not the query)

## Missing piece

`describe_ui` cannot see this at all — it reports the node as
`UNEVALUATED[live_query_rows]`, so the ONLY oracle for it was a screenshot,
which is why a durable blank row survived to a dogfood pass. Missing piece =
boot-ordering parity (the warm-boot path is not exercised anywhere: every
test starts from an empty db), plus the already-proposed "rendered items ==
query rows" oracle for `live_query` nodes, which would make this a headless
failure instead of a visual one.

## Remedy

OPEN 2026-08-07 — diagnosis only. Evidence: `shots/08.png`, `shots/09b.png`.
