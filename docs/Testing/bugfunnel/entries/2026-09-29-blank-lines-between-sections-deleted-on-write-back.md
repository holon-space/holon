---
id: 2026-09-29-blank-lines-between-sections-deleted-on-write-back
date: 2026-09-29
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  Blank lines an author put before a headline, between a headline's drawer
  and its body, before a page's first headline or at the end of a file were
  deleted, and a line of spaces was emptied, by the first write-back of an
  unchanged file, with no loss reported.
---

## Bug
Found by the org-faithful group B r3 verifier (`lane-logs/B3v-probe2.log`,
"blank line before the next headline"): `…:END:\nbody\n\n** Kid two` was
written back as `…:END:\nbody\n** Kid two`, `losses=[]`. Present on main
d4f426ffc965 (`lane-logs/B4-main-blank-lines.log`: 11 of 12 blank-line shapes
change on the first write-back; only a list body followed by one blank line is
kept). The 49 such lines in Martin's vault (7 files under `Projects/Holon/`)
all follow a list item, so the list terminator kept them by coincidence and
the vault write-back gate stayed at 0 changed lines.

## Root cause
The parser trims blank lines off both ends of a body (`trim_blank_lines` in
`crates/holon-org-format/src/parser.rs`, `extract_image_links`), and nothing
else recorded them. The renderer writes a blank line after a body only to
close a list (`body_needs_list_terminator`, `models.rs`).

## Missing piece
The keystone writes external files through the production renderer, so no
file it ingests carries an author's blank line, and the read-back check in the
renderer compares only a block's own text, which the blank lines are not part
of.

## Remedy
The parser records the bytes of the blank lines before a body, after a
headline's section (the end of the file included) and before a page's first
headline as the `_blank_lines` carrier (`BlankLines`, `models.rs`); the
renderer writes them back (`org_renderer.rs`, `end_with_blank_lines`, the end
of `render_document`; `render_headline_block`). A line of spaces or tabs keeps
its bytes. The org-ingest param builder forwards the carrier and the ingest
treats a changed carrier as a change (`holon-orgmode` `block_params.rs`,
`file_format.rs`). Pinned by `org_text_reads_back_as_written.rs`
(`an_unchanged_file_keeps_its_blank_lines`), holon-app
`org_store_org_round_trip.rs` (`blank_lines_survive_the_store`, both write
legs) and holon-orgmode `file_level_drawer_seam.rs`
(`blank_lines_survive_a_real_write_back`).

Open, with no loss raised (also listed in `docs/Reference/ORG_SYNTAX.md`):
blank lines between a body and a source block child, between two source
children, or between a headline with no body and its source child are not
kept; a list body followed directly by a headline or a source child gains one
blank line; a page created in Holon, or one with no keyword line before its
first headline, writes its pre-headline text after exactly one blank line; a
file without a final line break gains one. The keystone does not draw an Emacs-authored blank line.
