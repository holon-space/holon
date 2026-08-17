---
id: 2026-07-10-rule-infra-page-nav-duplicate-journals
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Rule-infra page nav (duplicate Journals page `13ab7a46…`): profile resolver
  panics on id-less rule-trigger row (`holon-profiles/src/lib.rs:776` "row has
  no entity `id`" on `{"_rowid":1,"name":"2026-07-10"}`) + creation slot
  panics "rowset has 4 disjoint root rows" (`row_origin.rs:190`); describe_ui
  returns -32603 for the whole page, pixels still render, no banner; recovers
  on nav-away
source_line: 880
---

## Bug

Rule-infra page nav (duplicate Journals page `13ab7a46…`): profile resolver
panics on id-less rule-trigger row (`holon-profiles/src/lib.rs:776` "row has
no entity `id`" on `{"_rowid":1,"name":"2026-07-10"}`) + creation slot
panics "rowset has 4 disjoint root rows" (`row_origin.rs:190`); describe_ui
returns -32603 for the whole page, pixels still render, no banner; recovers
on nav-away

## Missing piece

keystone never navigates to the journal-infra page; swallowed-bg-panic
invariant not in prod surface

## Remedy

FIXED (stream 2026-07-10): both panics were legal inputs treated as
impossible — (a) `resolve_with_computed`/`resolve_with_variants` now return
a visible `degraded-missing-id` profile (renders `⚠ unresolved row (no id):
…`) + loud WARN for id-less rows; (b) `resolve_creation_parent` returns
`None` (no slot offered) for multi-root non-sentinel forests + WARN. Tests:
`id_less_row_degrades_visibly_instead_of_panicking` (exact dogfood row),
`rule_infra_page_four_root_forest_offers_no_slot`,
`disjoint_roots_offer_no_slot` (was #[should_panic]); 10 pre-existing
row_origin tests unchanged. OPEN residue: fork A (action-rule blocks out of
display-render path) unchanged; generic swallowed-bg-panic→banner seam
deliberately not blanket-caught (needs its own workstream)
