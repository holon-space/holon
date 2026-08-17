---
id: 2026-07-16-wiki-links-dead-end-end-prod
date: 2026-07-16
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Wiki-links dead end-to-end in prod GPUI: ZERO blocks with non-null `marks`
  in the whole ingested-vault DB, `block_links` junction completely EMPTY, a
  `[[Journals]]` block renders as literal text with no link element (nothing
  clickable), backlinks impossible — the links-ruling Incr 0/1 pipeline (marks
  at ingest → junction → clickable render) is not active in this wiring
source_line: 827
---

## Bug

Wiki-links dead end-to-end in prod GPUI: ZERO blocks with non-null `marks`
in the whole ingested-vault DB, `block_links` junction completely EMPTY, a
`[[Journals]]` block renders as literal text with no link element (nothing
clickable), backlinks impossible — the links-ruling Incr 0/1 pipeline (marks
at ingest → junction → clickable render) is not active in this wiring

## Missing piece

live confirmation + extension of the open "org-drops-marks" row: ingest
resolves org `[[id][label]]` to plain text; agent-origin create with
`[[..]]` also never parses marks

## Remedy

ROOT-CAUSED + FIXED (2026-07-17): the ORG-INGEST symptom is NOT a marks-drop
— `block_raw.marks` IS populated in every mode (parser +
`build_block_params` + Loro `block_to_params` all emit marks). The break is
JUNCTION-ONLY and MODE-SPECIFIC: `block_links` is derived by
`SqlOperationProvider::block_link_statements`, which is wired into the
SINGLE-OP create/update/`set_field("marks")` paths but NOT into
`execute_batch_with_origin` — the Loro→SQL projection sink
(`BlockConsolidator::apply` → `execute_batch_with_origin`,
EventOrigin::Loro), i.e. the DEFAULT Loro/Upstream app wiring
(`crdt.enabled=true`; `TestEnvironmentBuilder` defaults loro ON). In that
mode boot ingest persists creates via `create_in_tree`→Loro and the
projector batch writes `block_raw` (marks included) but never `block_links`
→ junction EMPTY, backlinks impossible, wiki-link render has no target to
resolve. SqlOnly (`.without_loro()`) routes params through the single-op
path and was always correct. FIX: `execute_batch_with_origin` now derives
block_links from the `marks` param (and page re-resolve for Page-tagged
blocks) for every block create/update op, mirroring the single-op path
(crates/holon/src/core/sql_operation_provider.rs ~L2009). Red-first repro
`crates/holon-integration-tests/tests/wiki_link_ingest_marks_junction.rs`:
the `_loro` variant was RED (marks present, `block_links=[]`), `_sqlonly`
GREEN; both GREEN post-fix. Gates: holon-orgmode+holon-turso 157/157, holon
link tests (block_links_junction/live_edit_link_marks/create_page_from_link)
8/8. Residual (separate rows, NOT this fix): the AGENT-ORIGIN create-op
content path (MCP `create` with raw `[[..]]` in `content`) is not run
through `extract_inline_marks` — `operation_dispatcher` derives marks only
on `set_field("content")`, not on `create`; and a pre-marks-fix vault whose
files are hash-gated-unchanged won't backfill marks on re-boot (migration
gap).
