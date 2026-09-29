---
id: 2026-09-29-value-less-headline-drawer-key-voids-the-drawer
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A value-less `:NOTE:` line (no space after the key) in a headline's drawer
  makes orgize reject the whole drawer: the headline's `:ID:` is not read, a
  new id is minted, and the old drawer is kept as body text.
---

## Bug
Found by the org-faithful group B r8 lane (`lane-logs/B8-red.log` era probe):
`** K1\n:PROPERTIES:\n:ID: k1\n:NOTE:\n:END:` is written back with a new
`:PROPERTIES:` drawer holding a minted id, followed by the authored drawer as
text, `losses=[]`. The block loses its identity.

## Root cause
orgize's `node_property_node` requires whitespace after the key, so one
value-less line voids the headline's drawer; the file-level drawer has the same
grammar and is read by Holon's own `split_file_drawer` for this reason, the
headline drawer is not.

## Missing piece
No generator writes a value-less drawer key into a file.

## Remedy
When orgize rejects a headline's `:PROPERTIES:` drawer, the parser reads the
drawer named PROPERTIES that opens the section with its own line reader
(`properties_drawer`, `drawer_lines` in `parser.rs`): the `:ID:` and every key
are kept, `:NOTE:` reads as an empty value, the raw line is written back
unchanged. The per-block read-back (`check_block_reads_back`) now also
compares the re-read `:ID:` with the id the renderer meant to write; a
mismatch is a `WritebackLossy`. Pinned by `org_text_reads_back_as_written.rs`
(`a_drawer_key_with_no_value_keeps_the_drawer_and_its_id`) and
`models.rs` (`a_block_text_that_reads_back_with_another_id_is_a_loss`).
