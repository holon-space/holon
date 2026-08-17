---
id: 2026-07-21-duplicate-sidebar-pages-projects-holon-folder
date: 2026-07-21
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Duplicate sidebar pages: "Projects" ×2 and "Holon" ×2 — the folder-parent
  derivation (`get_or_create_by_name_chain`) mints a synthetic folder page
  (`block:db147710` Projects, `block:cb7d94d4` Holon) for nested
  `Dir/Child.org` files instead of resolving to the existing `Dir.org` index
  page id (`block:aef282e2` Projects), splitting a folder's identity across
  two blocks (ambiguous nav, split children); `block_tags` is dedup'd so this
  is genuine page-identity duplication, not a join artifact. PHASE-2 CONFIRMED
  REPRODUCIBLE ON CURRENT MAIN (fresh vault): a `Journals/2026-07-21.org`
  produced two "Journals" pages (canonical vs synthetic folder-parent) and two
  "2026-07-21" journals — not just legacy cruft.
source_line: 1085
---

## Bug

Duplicate sidebar pages: "Projects" ×2 and "Holon" ×2 — the folder-parent
derivation (`get_or_create_by_name_chain`) mints a synthetic folder page
(`block:db147710` Projects, `block:cb7d94d4` Holon) for nested
`Dir/Child.org` files instead of resolving to the existing `Dir.org` index
page id (`block:aef282e2` Projects), splitting a folder's identity across
two blocks (ambiguous nav, split children); `block_tags` is dedup'd so this
is genuine page-identity duplication, not a join artifact. PHASE-2 CONFIRMED
REPRODUCIBLE ON CURRENT MAIN (fresh vault): a `Journals/2026-07-21.org`
produced two "Journals" pages (canonical vs synthetic folder-parent) and two
"2026-07-21" journals — not just legacy cruft.

## Missing piece

no folder-tree ingest rung (`Dir.org` coexisting with `Dir/Child.org`) + no
one-page-per-folder-name invariant

## Remedy

open
