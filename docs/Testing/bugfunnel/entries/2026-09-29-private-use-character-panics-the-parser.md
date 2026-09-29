---
id: 2026-09-29-private-use-character-panics-the-parser
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A line starting with U+E000 (a private-use glyph, such as a Nerd Font icon)
  in a block's text or before the first headline made `parse_org_file` panic.
---

## Bug
Found by the org-faithful group B r6–r7 verifier (`lane-logs/groupB-r67-verify.md`,
F-A): 5 panic shapes, all in `take_keyword_lines`. Introduced by r7, which
assembled block text with U+E000 as an in-band marker for keyword lines.

## Root cause
The marker was assumed to occur in no text.

## Missing piece
No generator drew private-use characters, and nothing asserted that the parser
never panics.

## Remedy
The parser carries keyword-line places structurally (`SectionLine`,
`assemble_body` in `crates/holon-org-format/src/parser.rs`); no character in
the text has a meaning. Pinned by `org_text_reads_back_as_written.rs`
(`text_with_a_private_use_character_reads_back`, and the property
`any_block_text_parses_and_renders_without_a_panic` over arbitrary unicode
lines, private-use characters included).
