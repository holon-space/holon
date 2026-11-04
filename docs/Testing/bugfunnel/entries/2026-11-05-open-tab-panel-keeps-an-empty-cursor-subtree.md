---
id: 2026-11-05-open-tab-panel-keeps-an-empty-cursor-subtree
date: 2026-11-05
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  After two open_tab calls the main panel renders the cursor's page with an
  empty subtree for at least 10 s although the page's child row is in the store.
---

## Bug
Found by a paired A/B of the integration suite (change leg, 1 of 10 runs; base
leg 0 of 10), while the test waits moved from a 200 ms CDC quiet window to
`settle()`. Test
`frontend_suite::sidebar_modifier_click_open_tab_probe::many_open_history_rows_render_only_the_cursor_subtree`
panicked at `sidebar_modifier_click_open_tab_probe.rs:303:9`: panel ids
`[default-main-panel, <page_b>, __virtual:<page_b>]`, so page_b rendered with
its empty-children placeholder and never showed `block:probe-child-b`.

## Root cause
Not measured. The test's waits are not the cause: `settle()` waits until every
reactive watch applied its CDC (`crates/holon-frontend/src/reactive.rs:3815`
subscribes inside `reporting_to`; `crates/holon-turso/src/matview_manager.rs:836`
lists it as lagging until applied), child-b is created before `open_tab` in both
trees, and the test then polls the panel for 10 s
(`sidebar_modifier_click_open_tab_probe.rs:279`). A projection that is wrong for
the whole poll is a stuck projection: either a lost delta, or a fresh keyed
watch on a shared view (`matview_manager.rs:1490`) whose applied cursor stays
behind. Rate: 1 of 10 in the loaded A/B leg (1-min load about 9); 0 of 50 and
0 of 99 alone on the same tree (1-min load 5.8-28.6).

## Missing piece
The old 200 ms quiet window gave the projection time before `open_tab` and so
hid the race; no test records a watch's sent/applied cursor when a panel stays
wrong.

## Remedy
OPEN. Proposed known-red row and card text go to the steward; the projection
defect is not fixed in the lane that found it.
