---
id: 2026-07-12-journal-page-files-under-journals-absent
date: 2026-07-12
gap: COVERAGE
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  Journal page-files under Journals/ absent from left sidebar:
  folder-companion Journals.org inlines the same block IDs as plain headings,
  and its cold-boot reconcile strips the page-files Page tag (one-time
  last-writer-wins on scan order)
source_line: 967
---

## Bug

Journal page-files under Journals/ absent from left sidebar:
folder-companion Journals.org inlines the same block IDs as plain headings,
and its cold-boot reconcile strips the page-files Page tag (one-time
last-writer-wins on scan order)

## Missing piece

keystone never generates subdirectory page-files + folder-companion
duplication; cold-boot scan-order/base-seed timing unmodeled

## Remedy

Fork B FIXED (2026-07-13, B1) — now COVERED by an automated
composed-keystone repro, so it graduates from "outside a test":
`folder_companion_subdir_fileless_materializes_and_deinlines`
(`structural_pbt.rs`) seeds the real row-137 shape — `Journals.org` inlining
a `:Page:`-tagged FILELESS subdir date — through the real
`boot_and_seed_wide` boot and asserts, NON-inert, that after settle the date
page is MATERIALIZED into its own `Journals/2026-07-11.org`
(`inv-every-page-has-its-own-file`), the companion DE-INLINES it
(`inv-companion-has-no-child-page-headings`), the topology is legal
(`inv-no-page-under-non-page`), and disk==render(SQL) with no swallowed
ERROR — all green, zero block loss. Machinery: ADR-0025 sibling-grounded
union guard + B2 boot sweep (already landed) + the seed flip
(`assets/default/Journals.org` → `place: page(journals)`) +
no-pages-under-non-pages fail-loud in `name_chain` (propagated fail-loud
through `doc_id_to_path`'s 3 callers). Landed a REAL bug this repro flushed
out: `on_file_changed`'s `expected_block_count` UNDERFLOWED (`attempt to
subtract with overflow`, `file_sync_controller.rs:1539`) when a
`:Page:`-tagged inline child is BOTH gate-excluded AND consolidator-created
— fixed to a double-filter matching `expected_present_ids`. Two new tripwire
oracles registered in the composed catalog
(`inv-every-page-has-its-own-file`, `inv-no-page-under-non-page`); the
generator provably never produces a page under a non-page (pages are
seed-only, always at `no_parent`), so the oracle is the regression guard.
Fork A (ingest is_page() authority) still per its own stream. NB the FULL
random `general_e2e_composed_pbt` remains RED on the SEPARATE pre-existing
`journals ingest-data-loss` base red (auto-create rule blocks
`block:journals::{auto-create,action::0}` not landing in the SUT) —
unrelated to Fork B, documented as its own blocker
