---
id: 2026-07-21-delete-org-writeback-round-trip-defect
date: 2026-07-21
gap: COVERAGE
secondary: ORACLE
status: PARTIAL
summary: >-
  `convert_block_to_page` / delete org-writeback round-trip defect chain
  (explorer-confirmed 2026-07-21, root producer of the sidebar cluster above):
  (a) converted pages are written with ONLY `#+ID:`, never `#+TITLE:` — the
  title lives only in DB/Loro, so reingesting such a file yields an
  empty-content Page (Resources.org / Agentic DPL.org match this signature
  exactly); (b) a trailing `/` in a converted title is RETAINED in the DB
  title but STRIPPED from the filename (unsanitized, inconsistent, no
  warning); (c) an interior `/` splits into subdirectories (likely intended
  People/… namespace behavior) but leaves an orphan intermediate directory
  with no backing page; (d) `delete` removes the page block from the DB but
  leaves the `.org` file on disk → it reingests as an empty page.
source_line: 1080
---

## Bug

`convert_block_to_page` / delete org-writeback round-trip defect chain
(explorer-confirmed 2026-07-21, root producer of the sidebar cluster above):
(a) converted pages are written with ONLY `#+ID:`, never `#+TITLE:` — the
title lives only in DB/Loro, so reingesting such a file yields an
empty-content Page (Resources.org / Agentic DPL.org match this signature
exactly); (b) a trailing `/` in a converted title is RETAINED in the DB
title but STRIPPED from the filename (unsanitized, inconsistent, no
warning); (c) an interior `/` splits into subdirectories (likely intended
People/… namespace behavior) but leaves an orphan intermediate directory
with no backing page; (d) `delete` removes the page block from the DB but
leaves the `.org` file on disk → it reingests as an empty page.

## Missing piece

No round-trip test covers convert → write org → reingest → render (nor
delete → residual-file → reingest): the keystone has a `block_to_page`
transition but never re-ingests a just-written converted page, so the
title-loss and orphan-file cycles are ungeneratable. Secondary
ORACLE/fail-loud: the `/` strip-vs-retain inconsistency and the
delete-orphans-file both proceed silently (no warning/error). Remedy: write
`#+TITLE:` on convert (parse-don't-validate the title on reingest),
sanitize/echo `/` consistently, remove the org file on page delete; lock
with a convert→reingest→render round-trip rung + a
delete→no-residual-empty-page assertion.

## Remedy

PARTIAL 2026-07-21: (b) trailing-slash sanitizer LANDED on main; (a)
mitigated by the landed ingest-heal (filename is the title vehicle — no
#+TITLE: by design); (c) orphan intermediate dirs + (d) delete-leaves-file
OPEN pending rulings (page-hierarchy PARKED / destructive-action).
