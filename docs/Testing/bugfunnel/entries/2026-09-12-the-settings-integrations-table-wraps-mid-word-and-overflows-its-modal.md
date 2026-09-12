---
id: 2026-09-12-the-settings-integrations-table-wraps-mid-word-and-overflows-its-modal
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  In the Settings integrations table the Config column breaks "unconfigured"
  mid-word as "unconfigure/d" on every row, and the Setup column's buttons are
  clipped to "OP"/"CO" and run past the modal's right edge.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`), at the default window size with no resizing.

Two layout defects in one table, seen on every row including bundled ones:

**Config column.** Every row renders `unconfigured` broken across two lines
after the "e" — `unconfigure` / `d`. It is a single word broken at an arbitrary
point, not a hyphenated wrap, and it repeats down the whole table.

**Setup column.** The operation buttons paint a clipped two-letter fragment above
their label: `OP` above "Open", `CO` above "Configure…". On the `gcal` and
`gmail` rows a further element is cut off at the modal's right border, so the
column's content extends past the panel it lives in.

Screenshots `scratchpad/dogfood-uc/shots/03-settings-scrolled.png` and
`04-failing-connected.png`. Both show eight rows; the defects are on all of them.

The table is the surface this feature added an introduced-connection row to, so
its legibility is load-bearing for the disclosure work — a row a user is asked
to read carefully before switching a connection on.

## Root cause

Measured in a real 1512x900 window by the new rung below. The modal panel is
640px wide with 24px of padding, so the table gets a 590px content box. Three
findings, not one:

**1. Config wrap — the column's share.** `SETTINGS_ITEM_TEMPLATE` gave Config
`flex(1)` of a 2/1/1/fixed(80)/2 split, which resolves to **79.5px**.
"unconfigured" does not fit, a `table` `text` cell declares no `truncate`, and a
single word offers no break opportunity — so the layout broke it mid-word, into
a 52px-tall two-line cell.

**2. Setup overflow — an unbounded op label, not only the column's share.** The
Setup column resolved to 159.5px while gcal's op row measured **217px**. That
217px is dominated by one button: `integration.set_field` carries the display
name "Switch integration", and `op_button`'s label div had no width cap, so the
button grew to 96px to hold the words on one line. Widening the column alone
cannot absorb that — with the other four columns at the widths their own values
need (108 / 90 / 90 / 72) plus 32px of gaps, only 198px of the 590px box remain
for Setup. So this is a SECOND cause, and the teeth proof below shows each fix
is separately load-bearing.

**3. The `OP` / `CO` fragments are not a defect.** `op_button::render` paints an
icon line above the label line, and `fallback_short_label` supplies the first two
letters uppercased as that icon when the op has no glyph in `OP_ICONS`.
`open_default_view` and `begin_oauth` have none, so "OP" above "Open" and "CO"
above "Configure…" is the designed fallback, not clipped text. What WAS clipped
on the gcal row is the whole "Open" button: it painted at x=1067.5..1075.0, i.e.
cut off at the scroll container's edge — that is finding 2, seen from the side.

## Missing piece

PERCEPTION. `frontends/gpui/tests/settings_integrations_table_windowed.rs` and
`settings_integrations_row_op_alignment_windowed.rs` already open this exact
table in a real window, and `integrations_row_narrow_window_windowed.rs` already
reasons about it under constrained width. So the harness is in place and the
state is generatable; what is missing is an assertion about geometry rather than
about which texts are present — no test asks whether a cell's content fits the
cell, or whether any element's bounds exceed the modal's.

That assertion is expressible from the `BoundsRegistry` those tests already use,
which is what makes this worth pinning rather than filing as taste.

## Remedy

FIXED, red-first.

**The rung.** `frontends/gpui/tests/settings_integrations_table_fits_windowed.rs`
— a new windowed PBT that opens this table through the toolbar gear and judges
GEOMETRY off `BoundsRegistry`: every painted descendant of a
`table-cell-col-{k}-{row}` must lie inside that cell, nothing the table paints
may cross the modal panel's edges, and a column holding only text must paint one
line per cell (gpui wraps inside one text element, so a split word shows up as a
52px cell, not as a second element). It dumps the full per-column geometry on
every run, so the margins a column has left are readable without a failure.

**The fixes.**
- `crates/holon-app/src/integrations_section.rs` — `SETTINGS_ITEM_TEMPLATE`
  rebalanced from 2 / 1 / 1 / fixed(80) / 2 to **6 / 5 / 5 / fixed(72) / 11**,
  which resolves to 108 / 90 / 90 / 72 / 198 px. Config now holds
  "unconfigured" on one line and Setup covers the 193px the widest op row needs.
- `frontends/gpui/src/render/builders/op_button.rs` — the label div takes
  `max_w(64px)` and `text_center()`. Capped, not truncated: "Switch integration"
  wraps at the space into the button's second line instead of widening the
  button to 96px, and every single-word op name still fits on one line, so the
  cap can never break a word.

**Teeth proof.** Reverting only the template widths → RED
("column 4 (\"Setup\") row \"integration:gcal\" paints reactive_shell#64 at
x=891.5..1075.0, outside its cell's 891.5..1051.0"), with the Config cell back to
h=52.0. Reverting only the op-label cap → RED
("... paints op_button#63 at x=1029.0..1063.0, outside its cell's 853.0..1051.0").
Both restored byte-for-byte (sha256 unchanged); the rung is green, and
`settings_integrations_table_windowed`,
`settings_integrations_row_op_alignment_windowed` and
`integrations_row_narrow_window_windowed` stay green.
