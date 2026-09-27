---
id: 2026-09-28-block-text-line-starting-with-a-star-becomes-a-new-block
date: 2026-09-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block text line that starts with `* ` was written raw below the headline,
  so each read of the file cut the block's text there and added a new block.
---

## Bug
Found by the org-faithful verifier through `dense_query` → `dense_patch` (one
extra block per round trip over 3 rounds), then ruled by Martin as D230.a.
Block text `Shopping\n* milk` renders `* milk` at column 0, which org reads as
a headline: the block keeps `Shopping`, and a new block `milk` appears.

## Root cause
`render_headline_block` (`crates/holon-org-format/src/models.rs`) and the
page preamble in `OrgRenderer::render_document` wrote block text verbatim.
Org's comma escape was applied only inside source blocks.

## Missing piece
No keystone generator drew multi-line text with a `* ` body line. The
multi-line arm of `edit_content_strategy` drew `[a-z]…` lines only. Red with
the new arm: `lane-logs/B2-red-keystone.log` (minimal failing input
`BulkExternalAdd` content `"a Aa\n* a"`, per-tick reconcile finds a minted id
with no synthetic).

## Remedy
`CommaEscape::Body` (`crates/holon-org-format/src/comma_escape.rs`) escapes a
line org reads as a headline (stars, then a blank or the line's end), after
any commas; the parser removes one comma. The
same codec serves source blocks, which also fixed `,,* x` in a source block
reading back as `,,,* x`. Pinned by
`crates/holon-org-format/tests/org_text_reads_back_as_written.rs` and the
keystone's multi-line arm (`[reach] body-star-line`).
