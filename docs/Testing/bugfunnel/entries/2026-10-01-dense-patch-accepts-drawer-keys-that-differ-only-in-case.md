---
id: 2026-10-01-dense-patch-accepts-drawer-keys-that-differ-only-in-case
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A dense edit that wrote `:owner: me` and `:Owner: x` in one drawer applied
  as two properties, but org property keys are case-insensitive, so org reads
  one ambiguous property.
---

## Bug
Found by the Inc 6 round-3 verifier (`lane-logs/inc6rb3v-verify.md`, finding
C), lane decision Inc 6. Measured with `emacs -Q --batch` (30.2):
`org-entry-get` gives "x", `org-entry-properties` gives "me".

## Root cause
The duplicate-key refusal of `refuse_inexact` (`frontends/mcp/src/dense_patch.rs`)
compared drawer keys case-sensitively.

## Missing piece
The judge compared Holon's read with Holon's own parse, which keeps both keys;
the generator drew no case-variant key.

## Remedy
`refuse_inexact` refuses two keys equal under `to_lowercase` when one of their
lines is new in the edit: `{#a}: drawer keys :owner: and :Owner: are one org
property`. A vault drawer that already holds both still reads back. Pinned by
`a_text_org_writes_otherwise_is_refused_before_any_write` (`:Owner:` beside
`:owner:`, `:area:` + `:AREA:`), by `org_reads_otherwise` in the judge, and by
the generator's "case-variant drawer key" shape.
