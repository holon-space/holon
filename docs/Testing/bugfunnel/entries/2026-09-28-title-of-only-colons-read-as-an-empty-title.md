---
id: 2026-09-28-title-of-only-colons-read-as-an-empty-title
date: 2026-09-28
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A headline `* :::` read back as an empty title with no tags, and
  `* Foo :::` as `Foo`, so the colons were lost on the next write-back.
---

## Bug
Found by the org-faithful verifier (`lane-logs/Bv-probe2.log`, `emacs_table`).

## Root cause
The headline tag split took a token of only colons as a tag group that names
no tag.

## Missing piece
No row in the split's test table had a group that names no tag.

## Remedy
`Tags::split_org_headline` (`crates/holon-api/src/types.rs`) reads a token as
tags only when it names at least one tag. Pinned by
`org_text_reads_back_as_written.rs` (`a_tag_group_that_names_no_tag_is_title_text`).
The difference from org-element.el is declared in docs/Reference/ORG_SYNTAX.md.
