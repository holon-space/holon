---
id: 2026-09-26-dense-patch-folds-tags-of-a-tagged-row-into-its-title
date: 2026-09-26
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An unedited tagged row sent back through dense_query → dense_patch was
  retitled to `Title :tag:`, so its tag group landed in the content column
  and the org file grew a second copy of the tags.
---

## Bug
Found by code audit while adding tags and properties to `dense_patch` creates
(decision posting, D217). `dense_query` renders a tagged row as
`* Title :tag: {#3}`: the tag group comes before the alias token. The
harness test `dense_patch_tags_properties` (rung 1, before the fix) showed an
untouched round trip of two tagged rows reporting `"updated": 2`
(`lane-logs/inc6-red.log` in the lane).

## Root cause
`parse_dense` (`crates/holon-org-format/src/dense.rs`) hands the text to the
canonical org parser and strips the `{#alias}` token afterwards. With the
token at the end of the line, the org parser does not see the tag group as
trailing, keeps it as title text, and the planner diffs `Title :tag:` against
the projected `Title` — an `UpdateTitle` for every tagged row.

## Missing piece
The dense round-trip proptest (`crates/holon-org-format/tests/dense_roundtrip_proptest.rs`)
generated no tags and no drawer properties, and the planner PBT
(`frontends/mcp/tests/dense_patch_pbt.rs`) builds its parse synthetically, so
no test rendered a tagged row and parsed it back. The keystone
`DenseProjectionEdit` transition runs only in the live-MCP composition,
against a running app.

## Remedy
`parse_dense` splits the tag group off a token row's title with
`split_headline_tags`, the org parser's own tag grammar. The dense round-trip
proptest now generates tags and drawer properties (red on the old parser:
`"a :option:"` vs `"a"`), and the harness test pins the untouched round trip
at zero updates.
