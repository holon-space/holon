---
id: 2026-09-03-search-overlay-selected-row-subtitle-is-illegible
date: 2026-09-03
gap: PERCEPTION
secondary: null
status: PARTIAL
summary: >-
  The selected search hit paints its id subtitle in the muted grey meant for the
  light row background, leaving it unreadable on the teal selection fill.
---

## Bug

Found by the `dogfood-search` lane driving quick-open on the real vault copy.

Every hit row is a two-line cell: the label, and under it the block id in a
muted grey. On the SELECTED row the fill flips to the teal accent while the
subtitle keeps its muted grey, so the second line is dark-on-dark and cannot be
read at all. The label above it does switch to the selected foreground, so the
row is half-styled rather than unstyled.

Reproduced on every query driven this session — `Suppe`, `Compass`,
`NÄCHSTEN`, `%`, `\`, `e`, `en` (screenshots `07`, `08`, `10`, `12`, `13`,
`14`, `17`). The subtitle is the only thing distinguishing two hits with the
same label, and this vault has exactly that case: `Compass` matches two
different pages whose labels are identical and whose ids differ.

Same view, second defect: with hits in both sections the overlay's last content
row is cut by the panel's bottom edge (`07-search-suppe.png` — the second
`In content` row shows one clipped line and its subtitle is sliced through the
middle). The overlay grows to fit, then stops mid-row rather than at a row
boundary.

## Root cause

`render_search_overlay` (`frontends/gpui/src/search_ui.rs:288-504`) takes a
`SearchTheme` carrying both `muted_fg` and `selected_fg`
(`search_ui.rs:49-56`). The label reads `selected_fg` when the row is selected;
the subtitle reads `muted_fg` unconditionally. There is no
"muted-on-selection" colour in the theme for it to reach for.

## Missing piece

No formal invariant can express "these two colours have enough contrast", and
no layout test checks that the overlay's last row is whole. This is the
perception class the dogfood channel exists to cover — but the flow reaches it
only through the search overlay, which no `.feature` can address today (there
is no step vocabulary for opening quick-open or typing into it), so it cannot
be pinned as a replayable scenario either.

## Remedy

PARTIAL — the subtitle is fixed, the clipped last row is not.

The pointer fills a row with the same accent the keyboard selection uses, so
"carries the selection fill" is now ONE state: `SearchTheme::row_colors(filled)`
derives the row background, the title colour and the subtitle colour from one
bool, and hover is real state (`SearchUiState::hovered`) rather than a `hover`
style closure that could only repaint the row's own background. Without that,
hovering any unselected hit reproduced the same 1.11:1 ghost.

The filled row's subtitle reads `SearchTheme::selected_muted_fg`, derived per
theme by `holon_frontend::theme::muted_on_selection` (`theme.rs`): the
black/white pole with more contrast against the selection fill, mixed back
toward the fill only as far as the 4.5:1 body-text floor allows. Holon Light
measures 4.85:1 and Holon Dark 4.93:1, against 1.11:1 before.

Pinned by two tests, both of which fail on the old colour:

* `frontends/gpui/tests/quick_open_selected_row_contrast_windowed.rs` —
  windowed, opens quick-open, types a query and computes the ratio from the
  layout record's `painted_fg`/`painted_bg` (new fields on `ElementInfo`, which
  the GPUI tracker cascades so a text leaf reports the fill it inherits). Two
  rungs: `selected_hit_subtitle_…` for the keyboard selection and
  `hovered_hit_subtitle_…`, which drives a real `MouseMove` over an unselected
  hit.
* `theme::tests::selection_subtitle_clears_the_body_text_floor_in_every_theme`
  — the floor over every builtin theme's fill, asserting the exact registry
  count so a theme added without the floor cannot slip past.

STILL OPEN: the overlay's last content row is cut mid-row. Clip the results
list at a row boundary, or give it a max-height with a scroll container so a
partial row scrolls instead of being sliced.

Recording this flow at all needs an `open search` / `type into search` step
pair in the Gherkin vocabulary — logged as the vocabulary gap of this session.

## Dogfood re-run 2026-09-08

Still reproduces on the `search-fix` lane (port 8710, seeded throwaway vault),
now with pixel measurements from the captured frame (selected row spanning
y=352..452):

| element | darkest glyph pixel | highlight | contrast |
|---|---|---|---|
| row title | `rgb(20,20,19)` | `rgb(67,123,124)` | **3.83:1** |
| row subtitle | `rgb(76,118,116)` | `rgb(67,123,124)` | **1.05:1** |

WCAG AA wants 4.5:1 for body text and 3:1 for large text. The subtitle at
1.05:1 is a teal-on-teal ghost; the title misses AA for its size too. Visible
in every selected row across this session's evidence.

Screenshots: `lane-logs/dogfood-r2/02-page-ueber.png`,
`lane-logs/dogfood-r2/02-page-ueber-crop.png`.
