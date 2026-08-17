---
id: 2026-07-10-journals-page-query-never-worked-seeded
date: 2026-07-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Journals page query NEVER worked: seeded `filter name != null` referenced a
  non-existent `name` column — page query failed at execution on every boot;
  nothing asserted the seeded page queries actually execute (found by agent
  diagnostic during bug-3 fix)
source_line: 844
---

## Bug

Journals page query NEVER worked: seeded `filter name != null` referenced a
non-existent `name` column — page query failed at execution on every boot;
nothing asserted the seeded page queries actually execute (found by agent
diagnostic during bug-3 fix)

## Missing piece

no test executes the real seeded Journals.org page query end-to-end
(keystone seeds a bare shell)

## Remedy

FIXED alongside bug 3 (query migrated to real `content` columns); gap open:
a seeded-assets smoke test that executes every bundled page query
