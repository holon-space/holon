---
id: 2026-07-20-gpui-dogfood-org-link-writeback-emits
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  GPUI dogfood: org link writeback emits the `block:` scheme prefix on disk —
  resolved links are written `[[block:<uuid>][Label]]` instead of the
  documented bare-id `[[<uuid>][Label]]` (ORG_SYNTAX.md: "the renderer strips
  the scheme when writing"; "Renderer writes block.id.id() — the path part
  only"). Pervasive: seen on every page incl. `Charles Babbage.org` which I
  never edited. Also expands label-only `[[Ada Lovelace]]` to resolved id-form
  on writeback. Round-trips stably (parser treats `block:`-prefixed target as
  already-resolved) but violates the bare-id contract and org-tool interop.
source_line: 1036
---

## Bug

GPUI dogfood: org link writeback emits the `block:` scheme prefix on disk —
resolved links are written `[[block:<uuid>][Label]]` instead of the
documented bare-id `[[<uuid>][Label]]` (ORG_SYNTAX.md: "the renderer strips
the scheme when writing"; "Renderer writes block.id.id() — the path part
only"). Pervasive: seen on every page incl. `Charles Babbage.org` which I
never edited. Also expands label-only `[[Ada Lovelace]]` to resolved id-form
on writeback. Round-trips stably (parser treats `block:`-prefixed target as
already-resolved) but violates the bare-id contract and org-tool interop.

## Missing piece

Writeback/renderer path not asserted for exact on-disk link target form; a
golden org round-trip test should pin `[[<bare-uuid>][label]]`. (Caveat:
could be an intentional stability choice — flag for ruling; if intended,
update ORG_SYNTAX.md.)

## Remedy

OPEN
