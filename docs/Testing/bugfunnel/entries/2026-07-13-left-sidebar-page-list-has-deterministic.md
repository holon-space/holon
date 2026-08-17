---
id: 2026-07-13-left-sidebar-page-list-has-deterministic
date: 2026-07-13
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Left-sidebar page list has no deterministic order (GenFile019, 008, 017,
  007, 013, ... — neither alphabetical nor creation order; screenshot 03) —
  page lookup by eye is impossible at 20+ pages
source_line: 979
---

## Bug

Left-sidebar page list has no deterministic order (GenFile019, 008, 017,
007, 013, ... — neither alphabetical nor creation order; screenshot 03) —
page lookup by eye is impossible at 20+ pages

## Missing piece

sidebar Page query has no ORDER BY; no ordering oracle on rendered page list

## Remedy

OPEN — dogfood #5; fix = ORDER BY content (or recency) in
left_sidebar::src::0
