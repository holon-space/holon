---
id: 2026-07-22-report-martin-dogfooding-left-sidebar-pages
date: 2026-07-22
gap: ENVIRONMENT
secondary: PERCEPTION
status: UNCLASSIFIED
summary: >-
  Report (Martin dogfooding): left-sidebar Pages shows journal pages at TOP
  LEVEL instead of nested under a `Journals` parent; also a separate
  "incomplete pages list" report. Investigated against a COPY of the real
  71-file vault (fresh ingest, throwaway config, `HOLON_VAULT_ROOT=…`, queried
  over the embedded MCP `execute_raw_sql`). NEITHER symptom reproduces from a
  fresh ingest of current code: journal nesting is correct end-to-end — all 7
  journal-date page files (`Journals/2026-07-*.org`) carry
  `parent_id=block:journals` + the `Page` tag, `block:journals` is a root
  (`sentinel:no_parent`), and the sidebar tree builder
  (`OutlineTree::from_rows`, `crates/holon-api/src/render_eval.rs:252`) roots
  only the 9 folder/companion pages (Advice Dogfood, Areas×2, Denis, GitHub,
  Journals, People, Projects, Resources). `date_orphans=0` was observed at
  EVERY sample across a 40s vault-scale ingest settle (dates never flash to
  top level; `block:journals` present from the first sample). "Incomplete
  list" reproduces ONLY transiently: at real-vault scale the ingest settles
  slowly (pages grow 18→73 over ~40s, health-OK returns long before settle),
  so the sidebar looks partial until steady state (73/73 org-file doc-roots
  present, verified complete).
source_line: 1102
---

## Bug

Report (Martin dogfooding): left-sidebar Pages shows journal pages at TOP
LEVEL instead of nested under a `Journals` parent; also a separate
"incomplete pages list" report. Investigated against a COPY of the real
71-file vault (fresh ingest, throwaway config, `HOLON_VAULT_ROOT=…`, queried
over the embedded MCP `execute_raw_sql`). NEITHER symptom reproduces from a
fresh ingest of current code: journal nesting is correct end-to-end — all 7
journal-date page files (`Journals/2026-07-*.org`) carry
`parent_id=block:journals` + the `Page` tag, `block:journals` is a root
(`sentinel:no_parent`), and the sidebar tree builder
(`OutlineTree::from_rows`, `crates/holon-api/src/render_eval.rs:252`) roots
only the 9 folder/companion pages (Advice Dogfood, Areas×2, Denis, GitHub,
Journals, People, Projects, Resources). `date_orphans=0` was observed at
EVERY sample across a 40s vault-scale ingest settle (dates never flash to
top level; `block:journals` present from the first sample). "Incomplete
list" reproduces ONLY transiently: at real-vault scale the ingest settles
slowly (pages grow 18→73 over ~40s, health-OK returns long before settle),
so the sidebar looks partial until steady state (73/73 org-file doc-roots
present, verified complete).

## Missing piece

The "flat"-looking journals are subsumed by the render-layer tree-indent
INVERSION row above (PERCEPTION), FIXED in the ui-indent-bullet lane
2026-07-21 — a nested row that renders with no/inverted indent LOOKS
top-level. The keystone can't see either facet: it ingests a tiny synthetic
corpus that settles instantly (no slow-settle window) and has no GPUI window
/ Taffy layout to expose the visual indent. Parity remedy: a
real-vault-scale ingest+settle timing assertion (SLO p95) over the sidebar
`Page`-tree, plus a windowed indent-depth snapshot on a ≥2-level page
hierarchy.

## Remedy

NOT-REPRODUCED / already-covered 2026-07-22 — journal-nesting data + query +
tree builder all correct on current main; reported symptom explained by the
FIXED indent-inversion (PERCEPTION) and/or the still-OPEN
duplicate-folder-page class (F5, see next row) as a duplicate empty
"Journals" page in the live accumulated DB (prior row: "`Journals/` vault
dir spawns a duplicate empty Journals page"). No code fix warranted (no
persistent defect, no red-first repro possible from fresh ingest).
