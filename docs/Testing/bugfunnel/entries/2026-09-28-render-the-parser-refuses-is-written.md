---
id: 2026-09-28-render-the-parser-refuses-is-written
date: 2026-09-28
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A title starting with `[#1]` or `[#a]` was written as a priority cookie the
  parser refuses, so the whole page stopped ingesting.
---

## Bug
Found by the org-faithful group B r2 verifier (`lane-logs/B2v-probe2.log`,
`a_numeric_cookie_title_bricks_the_file`).

## Root cause
Nothing checked that the parser reads a rendered file back before it is
written.

## Missing piece
No test rendered a title the parser refuses.

## Remedy
`OrgRenderer::render_document` and `render_entitys` parse their output and
refuse it when the parser does; write-back leaves the file as it was and
raises `WritebackLossy` naming the refusal. Pinned by
`crates/holon-app/tests/org_store_org_round_trip.rs`
(`a_render_the_parser_would_refuse_leaves_the_page_readable`).
