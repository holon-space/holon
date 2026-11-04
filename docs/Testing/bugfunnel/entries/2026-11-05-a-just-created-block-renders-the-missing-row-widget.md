---
id: 2026-11-05-a-just-created-block-renders-the-missing-row-widget
date: 2026-11-05
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Under host load, a watch opened right after an awaited block.create renders
  the missing-row error widget: the block has no row yet for render_entity.
---

## Bug
Found by a paired A/B of the integration suite (2 of 10 runs in each leg, base
and change). Test
`store_suite::watch_ui::a_just_created_blocks_first_render_is_not_the_missing_row_widget`
panicked at `watch_ui.rs:296:13`: "a just-created block rendered the missing-row
error widget". The test awaits `create_block` and then opens the watch with no
wait for the row.

## Root cause
Not measured. `render_entity` (`crates/holon/src/api/block_domain.rs:224`)
returns `NoBlockRow` when `load_block_with_query_source` finds no row, and the
watcher paints the error widget for it (`crates/holon/src/api/ui_watcher.rs:390`).
So the awaited `block.create` (`crates/holon/src/testing/e2e_test_helpers.rs:136`)
returned before the row was readable by that lookup. Either the create op
returns before its row lands, or the lookup reads a projection that lags the
write. Rate: 2 of 10 in each loaded leg (1-min load about 9).

## Missing piece
The test's doc comment assumes an awaited create puts the row in the store
before the first render. No test pins when `block.create` returns relative to
its row being readable, and the suite's single runs on an idle host do not
reach the race.

## Remedy
OPEN. Proposed known-red row and card text go to the steward; the defect is not
fixed in the lane that found it.
