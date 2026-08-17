---
id: 2026-07-20-gpui-dogfood-inline-links-break-onto
date: 2026-07-20
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  GPUI dogfood: inline `[[page]]` links break onto their OWN line in rendered
  (unfocused) blocks despite ~1100px free width. "Owner is [[Ada Lovelace]]
  and reviewer [[Charles Babbage]]" renders stacked as "Owner is" / "Ada
  Lovelace" / "and reviewer" / "Charles Babbage"; focusing the block collapses
  it back to one correct inline line. Reproduced across Apollo, Ada Lovelace,
  Charles Babbage, TrailingSpaces, journal pages (screenshots 02-06). Marks
  are byte-correct in SQL — purely a rich-text line-layout defect where each
  link run forces a wrap.
source_line: 1035
---

## Bug

GPUI dogfood: inline `[[page]]` links break onto their OWN line in rendered
(unfocused) blocks despite ~1100px free width. "Owner is [[Ada Lovelace]]
and reviewer [[Charles Babbage]]" renders stacked as "Owner is" / "Ada
Lovelace" / "and reviewer" / "Charles Babbage"; focusing the block collapses
it back to one correct inline line. Reproduced across Apollo, Ada Lovelace,
Charles Babbage, TrailingSpaces, journal pages (screenshots 02-06). Marks
are byte-correct in SQL — purely a rich-text line-layout defect where each
link run forces a wrap.

## Missing piece

No headless oracle can express inline line-wrapping of a rich-text run;
needs a windowed T3 layout snapshot asserting a link-bearing block lays out
on the expected number of lines (1 when it fits). Likely in the GPUI
rich-text run builder (link run treated as block-level / forces break).

## Remedy

OPEN
