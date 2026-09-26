---
id: 2026-10-01-dense-patch-page-title-inside-a-block-is-applied
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A `#+TITLE:` line inside a verse or comment block of a dense_patch row was
  applied: the store kept the page title, the org file then read the line as
  the page title.
---

## Bug
Found by the rb4 verifier of the decision Inc 6 lane: after the edit the store
held the page title `dense_page_17`, the org file read `inverse`.

## Root cause
`refuse_page_keywords` checked only the row's keyword lines (the parser
carrier), not lines inside blocks. org-get-title reads `#+TITLE:` inside
every block.

## Missing piece
The engine judge compared rows only, never the page title of store and file.

## Remedy
`refuse_page_keywords` (`frontends/mcp/src/dense_patch.rs:205`) checks every
line of the row as org writes it to its file, blocks included, that the
projection did not show. Judge check `Session::page_title_agrees`
(`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs:622`) and
the verse/comment variants of
`a_text_org_writes_otherwise_is_refused_before_any_write`.
