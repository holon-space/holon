---
id: 2026-10-01-dense-patch-accepts-a-tag-org-reads-as-title-text
date: 2026-10-01
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  A dense edit that adds the tag `:a-b:` applies "exactly" in Holon (store tag
  `a-b`, file `* Plan :a-b:`), but org reads `* Plan :a-b:` as the title
  "Plan :a-b:" with no tags. Ruling pending (decision D9).
---

## Bug
Found by the Inc 6 round-3 verifier (`lane-logs/inc6rb3v-verify.md`, finding
B), lane decision Inc 6.

## Root cause
Measured with `emacs -Q --batch` (30.2): org tag characters are the ASCII
alphanumerics, `_@#%`, and the Unicode classes L*, M*, Nd, Nl. `-` is not one
of them. Holon reads `-` in a tag on purpose: its orgize fork exists for that,
and Martin's vault holds 28 `:human-only:` tags. A tag with `.` (`:a.b:`) is
title text in both org and Holon. `Tags::unrepresentable_char`
(`crates/holon-api/src/types.rs`) knows only `,`, `:` and whitespace.

## Missing piece
The judge compared Holon's read of the file with Holon's own parse, which
shares the tag grammar, so a dialect difference with org was invisible.
`tags_read_as_org_reads` (`dense_patch_engine_exact.rs`) now reads the tags
independently, and encodes `-` as the Holon dialect until D9 is ruled.

## Remedy
None yet. Decision D9: either Holon keeps `-` as a dialect extension (and
documents it in `docs/Reference/ORG_SYNTAX.md`), or dense_patch refuses tags
org cannot read. `Tags::unrepresentable_char` stays unchanged until the ruling.
