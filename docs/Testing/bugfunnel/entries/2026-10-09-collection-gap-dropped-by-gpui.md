---
id: 2026-10-09-collection-gap-dropped-by-gpui
date: 2026-10-09
gap: PERCEPTION
secondary: ORACLE
status: FIXED
summary: >-
  GPUI drew an accepted collection `gap:` only for a virtualized `list` (floored at 2 px); every
  other layout drew 2 px, a column-nested (eager) stacked collection 0 px, a board 16 px between
  lanes, and a horizontal list in a panel stacked its items.
---

## Bug
Found by the adversarial verifier of the C1 lane (`c1r4-verify.md`, D-V2, D-V3) by reading
the render paths after the parse was made strict.

## Root cause
`frontends/gpui/src/views/reactive_shell.rs` read the gap through
`.filter(|l| l.name() == "list")` and `px(g.max(2.0))` in both its branches;
`frontends/gpui/src/render/builders/column.rs` `eager_collection_div` gave the `Stacked` arm no
`.gap`; `board.rs` used its `LANE_GAP_PX` constant; `table.rs` hard-coded its row gap; the
virtualized `gpui::list` cannot lay items side by side, so `horizontal:` was dropped there.

## Missing piece
The headless property checked the parsed `CollectionVariant`, not the pixels; no windowed test
measured item spacing.

## Remedy
Both shell branches and the eager div draw the variant's gap for every layout; a collection that
flows horizontally renders through the eager div; the board draws its `gap` between lanes; the
columnar table between rows. `LayoutSpec::flows` limits `horizontal:` / `wrap:` to `list`; the
parse refuses them elsewhere. Default gaps are the main-panel spacing users saw (list 4, tree /
outline / table 2, table_columnar 4, columns 16, board 16). Windowed test
`frontends/gpui/tests/collection_gap_windowed.rs`; red log
`lane-logs/c1h/dv2-dv3-windowed-red.log`.
