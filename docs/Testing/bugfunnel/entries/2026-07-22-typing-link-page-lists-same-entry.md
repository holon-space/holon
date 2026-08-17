---
id: 2026-07-22-typing-link-page-lists-same-entry
date: 2026-07-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Typing `[[` to link a page lists the SAME entry TWICE in the autocomplete
  popup (Martin dogfooding) — once as a block, once as a page. Root cause:
  `QueryEngine::search_link_candidates`
  (`crates/holon/src/api/query_engine.rs:72`) `UNION ALL`'d branch-1 = ALL
  blocks matching content with branch-2 = Page-tagged blocks matching content;
  a Page-tagged block matches both branches, so it appeared twice. Sibling
  `quick_open_search` right below did it correctly (excludes Page from the
  content branch).
source_line: 1101
---

## Bug

Typing `[[` to link a page lists the SAME entry TWICE in the autocomplete
popup (Martin dogfooding) — once as a block, once as a page. Root cause:
`QueryEngine::search_link_candidates`
(`crates/holon/src/api/query_engine.rs:72`) `UNION ALL`'d branch-1 = ALL
blocks matching content with branch-2 = Page-tagged blocks matching content;
a Page-tagged block matches both branches, so it appeared twice. Sibling
`quick_open_search` right below did it correctly (excludes Page from the
content branch).

## Missing piece

The `[[` link-autocomplete search (`search_link_candidates`) is a
UI-adjacent query capability the keystone never drives — no transition opens
the link popup and issues the search, and no invariant asserts
link-candidate id uniqueness.

## Remedy

FIXED 2026-07-22 — branch-1 now excludes Page-tagged blocks (`AND id NOT IN
(SELECT block_id FROM block_tags WHERE tag = 'Page')`), making the two
branches disjoint (fix at query semantics, not a symptomatic `.dedup()`).
Red-first proven by new engine e2e
`crates/holon/tests/link_candidate_dedup_e2e.rs` (pre-fix ids `[rust_note,
rust_page, rust_page]` → post-fix each id once, non-page content still
listed). Keystone-repro path (not built): add a driver rung that calls
`search_link_candidates` after creating a Page whose title matches the
filter, with an invariant that the returned id-set is duplicate-free —
closes the COVERAGE gap.
