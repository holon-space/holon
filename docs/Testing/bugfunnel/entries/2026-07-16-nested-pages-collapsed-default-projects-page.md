---
id: 2026-07-16-nested-pages-collapsed-default-projects-page
date: 2026-07-16
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Nested pages are NOT collapsed by default: Projects page renders 951 tree
  items — every nested page (Holon → Frontends → TUI/GPUI…) fully expanded on
  first paint (`Block::default().collapsed=false`, org parser "absent means
  expanded")
source_line: 830
---

## Bug

Nested pages are NOT collapsed by default: Projects page renders 951 tree
items — every nested page (Holon → Frontends → TUI/GPUI…) fully expanded on
first paint (`Block::default().collapsed=false`, org parser "absent means
expanded")

## Missing piece

expected-UX default (collapse nested pages) unimplemented/unasserted; no
first-paint render-shape check

## Remedy

OPEN — screenshot /tmp/dogfood-0716-logs/shot-projects.png
