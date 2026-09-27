---
id: 2026-09-27-headline-title-ending-in-colons-loses-text-or-tags
date: 2026-09-27
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A headline title that ends in colons before its tag group lost text or its
  tags on render -> parse (`Pick ::` read back as `Pick`, `Pick:` + tags read
  back with no tags), and an Emacs-written `* Pick :: :decision:` lost `::` at
  ingest.
---

## Bug
Found by the org-faithful probe (`crates/holon-org-format/tests/org_faithful_probe.rs`,
`lane-logs/org-faithful-probe-red.log`), outside the keystone.

## Root cause
The orgize fork's tag scanner (`headline_tags_node`) accepts empty and
whitespace items inside a tag group and needs whitespace before the first
colon. Emacs (`org-element-headline-parser`) takes the tags as the last
blank-separated `:…:` token only.

## Missing piece
The keystone model splits titles with `split_headline_tags`, the same
grammar as the parser, so it agreed with the defect. Its content generator
never draws a title that ends in colons before a tag group.

## Remedy
The parser re-splits the headline text the Emacs way (`split_title_and_tags`
in `crates/holon-org-format/src/parser.rs`); `split_headline_tags` uses it,
so the model follows. Red `lane-logs/B-red-h2a.log`, green
`lane-logs/B-green-h2a.log`. Vault differential over 1073 files: 0 changed
items (`lane-logs/B-vault-differential*.log`). A title that itself ends in a
tag group with no tags on the block still reads back as tags (org has no
escape); the render now reports it as a loss and write-back raises a
`WritebackLossy` condition for the file.
