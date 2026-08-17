---
id: 2026-07-22-sidebar-pages-render-sort-key-creation
date: 2026-07-22
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Sidebar pages render in sort_key (creation/ingest) order, not the declared
  SQL ORDER BY content ASC — the tree() render's sortkey:col("sort_key")
  overrides the query sort (index.org:11 vs :14); alphabetical intent lost
  (Scratch/Target Page after Zulu). List is complete and scrollable on current
  main
source_line: 1093
---

## Bug

Sidebar pages render in sort_key (creation/ingest) order, not the declared
SQL ORDER BY content ASC — the tree() render's sortkey:col("sort_key")
overrides the query sort (index.org:11 vs :14); alphabetical intent lost
(Scratch/Target Page after Zulu). List is complete and scrollable on current
main

## Missing piece

no invariant that a tree/list render's child order matches its effective
declared sort; tree sortkey silently overrides SQL ORDER BY

## Remedy

open (sidebar-sort fix lane in flight 2026-07-22)
