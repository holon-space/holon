---
id: 2026-09-29-source-block-line-loses-two-commas-on-read
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A source-block line `,,* x` was read as `* x`: the parser removed the
  escape comma twice, so the next write-back wrote `,* x`.
---

## Bug
Found by the org-faithful group B r9 lane while writing the R3 test: org
reads `,,* x` inside `#+BEGIN_SRC` as `,* x`; Holon read `* x`. The text
changed on a round trip, `losses=[]`.

## Root cause
orgize's `SourceBlock::value()` already drops the escape comma, and the parser
then applied `CommaEscape::Source.unescape` to that value.

## Missing piece
No generator wrote a source line with two leading commas.

## Remedy
The parser reads the raw `BLOCK_CONTENT` text of the source block and
unescapes it once. Pinned by `org_text_reads_back_as_written.rs`
(`a_source_line_loses_one_comma_and_changed_text_is_escaped`).
