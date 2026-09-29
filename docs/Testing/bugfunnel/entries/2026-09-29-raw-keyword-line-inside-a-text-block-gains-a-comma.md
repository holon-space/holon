---
id: 2026-09-29-raw-keyword-line-inside-a-text-block-gains-a-comma
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A raw `#+KEYWORD:` line inside a text block (`#+begin_example`, ...) written
  in Emacs is written back with a comma in front, with no loss reported.
---

## Bug
Found by the org-faithful group B r8 lane: `#+begin_example\n#+TODO: X | Y\n#+end_example`
is written back as `#+begin_example\n,#+TODO: X | Y\n#+end_example`, `losses=[]`.
The text reads back the same; the bytes of an unchanged file change.

## Root cause
The D230.a escape (`CommaEscape::Body`) is decided per line and does not know
that the line stands inside a text block, where org reads no keyword.

## Missing piece
No generator writes an unescaped `#+` line inside a text block.

## Remedy
Ruling R3 (r9): a line org reads literally keeps its authored bytes while the
text is unchanged. The parser keeps the authored text of a block in the
`_authored_text` carrier when the renderer would write the same text with
other bytes (a headline body, the page preamble, a source block); the
renderer writes it while it still reads as the block's text, and the escaped
form once Holon changes the text. Inside a drawer (`:LOGBOOK:`) org removes no
comma, so only a headline-shaped line is escaped there (`comma_escape.rs`,
`drawer_interiors`). Pinned by `org_text_reads_back_as_written.rs`
(`an_authored_literal_keyword_line_keeps_its_bytes`,
`typed_text_inside_a_drawer_is_not_comma_escaped`,
`a_source_line_loses_one_comma_and_changed_text_is_escaped`).
