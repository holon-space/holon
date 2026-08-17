---
id: 2026-07-21-journals-feed-renders-file-insertion-order
date: 2026-07-21
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Journals feed renders in file/insertion order, not the declared `ORDER BY
  content DESC` / `sortkey:"-content"` — the newest date (2026-07-21) shows
  LAST; the declared DESC sort is not applied to the rendered `list` children.
  Secondary (P3, folded in): a trailing-slash `* 2026-07-12/` (unhealed
  pre-fix convert-boundary artifact, same no-migration class as F1/F4) and two
  "Journal Auto-Create" automation blocks also pollute the human-facing feed.
source_line: 1087
---

## Bug

Journals feed renders in file/insertion order, not the declared `ORDER BY
content DESC` / `sortkey:"-content"` — the newest date (2026-07-21) shows
LAST; the declared DESC sort is not applied to the rendered `list` children.
Secondary (P3, folded in): a trailing-slash `* 2026-07-12/` (unhealed
pre-fix convert-boundary artifact, same no-migration class as F1/F4) and two
"Journal Auto-Create" automation blocks also pollute the human-facing feed.

## Missing piece

no invariant that a `list` render's child order matches its declared
`sortkey`/`ORDER BY`; and a rung mixing non-date siblings (automation
blocks) into the feed

## Remedy

open
