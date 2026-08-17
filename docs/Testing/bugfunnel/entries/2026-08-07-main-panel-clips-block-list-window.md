---
id: 2026-08-07-main-panel-clips-block-list-window
date: 2026-08-07
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The main panel clips its block list ~22% of the window height above the
  window bottom, with no scrollbar or any affordance signalling hidden
  content.
source_line: 1168
---

## Bug

(overnight dogfood-explorer, same session) **The main panel clips its block
list ~22% of the window height above the window bottom, with no scrollbar or
any affordance signalling hidden content.** A block just past the cut is
absent from the paint entirely — `describe_ui` reports `rendered_text
block:42a84072-… ABSENT: not_painted` while every sibling in the same dump
carries real bounds and `visible` — despite the block being present in SQL,
on disk (`* EIGHT` in `Deep.org`) and in the widget tree. It survives a full
restart in that state. The list DOES scroll (a `scroll {dy:-300}` brings it
to `x=376 y=362 visible`); nothing indicates to the user that there is
anything to scroll to. Compounding it, the entire region below the "Linked
references" divider — roughly the bottom quarter of the window — is empty,
so the vertical space the content was clipped for is not being used by
anything.

## Root cause

overnight dogfood — the main panel clips its block list roughly 22% of the
window height above the window bottom, with NO scrollbar or any other
affordance signalling hidden content. A block sitting just past the cut is
absent from the paint entirely (`describe_ui` reports it `ABSENT:
not_painted` while every sibling is `visible`) even though it is in SQL, on
disk and in the widget tree; scrolling brings it back, so the list DOES
scroll — nothing tells the user there is anything to scroll to. Meanwhile
the whole region below the "Linked references" divider sits empty, so the
space the content was clipped for is not being used)

## Missing piece

No headless assertion expresses "content the user cannot see and is not told
about"; the block is fully correct in every non-visual surface, so
SQL/disk/widget-tree oracles all pass. This is exactly the class the
windowed rungs exist for. Missing piece = a layout assertion that the
painted row set covers the projected row set OR a scroll affordance is
present — `describe_ui`'s `visible`/`not_painted` geometry already carries
everything needed to express it mechanically.

## Remedy

OPEN 2026-08-07 — diagnosis only. Evidence: `shots/07.png`, `shots/08.png`.
