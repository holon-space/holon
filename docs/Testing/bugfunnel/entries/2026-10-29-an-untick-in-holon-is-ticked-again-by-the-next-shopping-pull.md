---
id: 2026-10-29-an-untick-in-holon-is-ticked-again-by-the-next-shopping-pull
date: 2026-10-29
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  An un-tick in Holon of a shopping item the app holds ticked was never pushed,
  and the next pull ticked it again without a word; a tick in Holon never
  reached the app either.
---

## Bug
Found while planning the shopping tick push, by reading the reconciler: the
sidecar declared `checked` a latch column, and the request mapping had no
command for a tick change. An un-tick in Holon stayed local; the next pull saw
the app's tick, which a latch always takes, and set `checked` back to 1. No
disclosure, no log line.

## Root cause
`assets/integrations/shopping.yaml` `list_sync.latch_columns: [checked]`. A
latch lowers only when the peer itself un-set the column, and Holon's own
changes to it are never pushed. The request mapping refused to encode a tick
because the app's wire for it had not been captured.

## Missing piece
COVERAGE: the keystone's remote-list transitions had no un-tick in Holon of an
item the app holds ticked. ORACLE: its `TickLocally` model expected a Holon
tick to stay local, so the peer-state invariant accepted a tick that never
reached the app.

## Remedy
`checked` is a merge column (three-way against `remote_base`), so a Holon
change of it is pushed and the app's own change is taken. The request mapping
sends the app's own wire: a tick is `cmd del` of the active entries plus
`picked add`, an un-tick `picked del` plus `cmd add`. The reconciler compares
`1` and `true` as one flag value, so a stored base of either spelling settles.
A commit the app fails names each unsent item and keeps Holon's values for the
next round.

Tests (red before, green after):
- `shopping_pull_mock.rs`
  - `a_tick_in_holon_reaches_the_app_as_its_own_check_wire`
  - `an_untick_in_holon_reaches_the_app_and_the_next_pull_keeps_it_unticked`
  - `an_untick_the_app_fails_is_disclosed_by_name_and_kept`
- keystone: transition `UntickLocally`, and `TickLocally` now expects the
  tick in the app; hand-authored cases
  `remote-list-a-tick-in-holon-reaches-the-app` and
  `remote-list-an-untick-in-holon-reaches-the-app-and-stays`.
