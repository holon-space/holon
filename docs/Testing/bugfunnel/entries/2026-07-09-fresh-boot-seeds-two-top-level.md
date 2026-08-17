---
id: 2026-07-09-fresh-boot-seeds-two-top-level
date: 2026-07-09
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Fresh boot seeds TWO top-level "Journals" Pages: `block:journals` (empty,
  from disk `Journals.org` `#+ID: journals`) AND `block:a6249a34-…`
  (programmatic first-boot seed carrying the journal machinery — child
  query-source block + "Journal Auto-Create" rule). Both are `["Page"]`-tagged
  and both render in the sidebar as identical "Journals" rows. The auto-create
  action targets `parent_id:"block:journals"` (the empty disk page), not its
  own machinery page
source_line: 874
---

## Bug

Fresh boot seeds TWO top-level "Journals" Pages: `block:journals` (empty,
from disk `Journals.org` `#+ID: journals`) AND `block:a6249a34-…`
(programmatic first-boot seed carrying the journal machinery — child
query-source block + "Journal Auto-Create" rule). Both are `["Page"]`-tagged
and both render in the sidebar as identical "Journals" rows. The auto-create
action targets `parent_id:"block:journals"` (the empty disk page), not its
own machinery page

## Missing piece

first-boot journal seeding and the disk `Journals.org` writeback each mint a
"Journals" Page; the keystone runs neither dual seed path; no invariant
forbids two identically-named forest-root Pages

## Remedy

open — root-caused: the disk `Journals.org` has no `#+ID:`, so
`parse_org_file` falls back to `file:Journals.org` (`set_page(true)`)
instead of resolving to the fixed shell `block:journals`
(`build_default_layout_blocks`), yielding two Pages. The naive one-line fix
(`#+ID: journals` in the asset) DEDUPES the page but was REJECTED after live
verification: because the org parser cannot let a *document* own
heading-parsed source blocks, the machinery
(`journals::src/render/trigger/action`) then renders as raw literal-text
rows on the canonical Journals landing page (9 rows incl. `from block
filter…`, `SELECT date('now')`, `block.create(…)`) — worse than the
duplicate. Proper fix = the programmatic worker-model seed
(`frontends/holon-worker/src/seed.rs:171-196` pattern: the `block:journals`
shell owns `src::0`/`render::0` directly, no org document, no duplicate),
landed alongside the keystone ref (`seed_booted_layout_into_ref`) so the
boot auto-create doesn't diverge the ref
