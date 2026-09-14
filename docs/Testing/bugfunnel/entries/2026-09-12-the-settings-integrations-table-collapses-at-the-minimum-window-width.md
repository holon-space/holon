---
id: 2026-09-12-the-settings-integrations-table-collapses-at-the-minimum-window-width
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  At the app's own minimum window width the Settings integrations table wraps
  every header and cell to two or three characters per line, so a row reads as
  a vertical column of syllables rather than as a row.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6`, booted at
`HOLON_INITIAL_WINDOW_SIZE=300x600` (300 = `MIN_WIDTH`,
`frontends/gpui/src/window_state.rs:24`).

The table's five columns keep their flex shares of a modal about 250 points
wide. Every column becomes too narrow for its own words, and because the cells
wrap rather than truncate, each one becomes a vertical stack:

- The header row reads `Int / eg / rat / ion`, `C / on / fig`, `St / at / us`,
  then `Enabled` and `Setup` at full width.
- `jsonplaceholder` reads `jso / npl / ace / hol / der`.
- `unconfigured` reads `un / co / nfi / gu / re / d`.
- `Pending` reads `Pe / ndi / ng`.

One row occupies most of the viewport height, the Setup column is clipped at
the right edge, and the columns no longer line up with their headers because
each cell's height differs.

This is not the same defect as the disclosure lines eliding to nothing at the
same width — that is recorded as
`2026-09-12-the-hosts-disclosure-line-elides-a-host-that-fits-its-column`. This
entry is about the table as a whole, including the bundled rows that carry no
disclosure lines at all.

Evidence under `scratchpad/dogfood-uc-recheck/shots/`:

- `C-01-table-300.png` — three rows, every cell a stack of syllables.
- `C-02-fixturebox-300.png` — the header row broken the same way.
- `C-04-crop.png` — the introduced row at that width.
- `B-01-table-800.png` — the same table at 800 wide, still clean, which fixes
  the breaking point somewhere between 300 and 800.

## Root cause

Not root-caused to a line. The columns are declared as
`flex(6) / flex(5) / flex(5) / fixed(72) / flex(11)` in
`crates/holon-app/src/integrations_section.rs:117-123`. Flex shares with no
minimum column width and no table-level "below this width, change shape" rule
give every column a proportional slice of whatever there is, including slices
narrower than a word.

## Missing piece

`integrations_row_narrow_window_windowed.rs` is the one narrow-window rung and
it uses 800x900 — above the breaking point, so it passes and always has. No
rung opens a window at the width the app itself declares as its floor, so the
whole range between `MIN_WIDTH` and 800 is unjudged.

## Remedy

Open, and it is a design question before it is a fix: either the table gets
minimum column widths and scrolls horizontally, or it changes shape below some
width (one row per connection becomes a stacked card). Whichever is chosen, the
narrow rung should be re-pointed at `MIN_WIDTH` rather than 800 so the floor is
the thing under test.

## Fix

The table's flex weights now carry the container width they were AUTHORED
against. `crates/holon-app/src/integrations_section.rs` declares
`min_width: 584` — the modal panel's 640px less its padding and border — and
`crates/holon-frontend/src/shadow_builders/table.rs` turns each column's share
of that width into that column's FLOOR (`column_minimums`). Flex shares are
proportional all the way down; a floor makes the row WRAP once the columns no
longer fit side by side, instead of shrinking every one of them past its own
words. At the authored width and above nothing about the layout changes.

`parse_min_width` refuses a non-numeric or non-positive value rather than
guessing. Absent is legal and means no floor.

Covered by `frontends/gpui/tests/settings_integrations_table_fits_windowed.rs`,
which sweeps 300..1512px through `resize_window`.

- RED `lane-logs/red3-settings_integrations_table_fits_windowed-1789381398.log`.
- GREEN `lane-logs/sweep-1789389884.log` — five distinct geometries over the
  seven widths (900, 1200 and 1512 coincide because the modal panel is capped at
  640px, so every window at or above the cap hands the table the same
  container).

**What the green does NOT cover, at the width this entry is named for.** At 300
and 360 the sweep observes `rows=0 cols=0`: no data row is painted at all, so at
the two narrowest widths the claim rests on HEADER geometry alone — the headers
wrap into whole words instead of breaking mid-word, which is the collapse this
entry reports, and which the live pass confirmed. The per-CELL half of the claim
is judged from 480px up.

The rows are not merely below the fold; they cannot be reached, which is the
separate defect recorded in
`2026-09-14-the-settings-modal-clips-its-lower-sections-at-the-minimum-window-width`.
Until that one is fixed, no rung can assert cell geometry at 300 or 360, because
there is no cell to measure.
- TEETH `lane-logs/teeth-minwidth-settings_integrations_table_fits_windowed-1789390428.log`
  — `min_width` removed; 6 violations over the 7 widths, the first being the
  "Integration" header broken across lines at 360px; restored byte-for-byte.

Note: the first teeth run of this fix (12:18) predates the harness fix recorded
in `2026-09-14-a-windowed-resize-never-reached-the-window-so-width-sweeps-measured-one-layout`
and was therefore judging the boot layout seven times. The run cited above is
the one against a sweep that really sweeps.
