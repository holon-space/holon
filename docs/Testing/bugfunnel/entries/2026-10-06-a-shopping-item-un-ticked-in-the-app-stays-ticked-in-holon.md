---
id: 2026-10-06-a-shopping-item-un-ticked-in-the-app-stays-ticked-in-holon
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An item a person un-ticks in the shopping app to put it back on the list
  stays ticked in Holon for ever, because the `checked` latch never lowers.
---

## Bug
Found by the round-4 verifier of the shopping-edits lane (D93.a), defect 3 of
`lane-logs/shopping-edits-verify2.md`, by reading the reconciler; no test
showed it. The `checked` latch (`list_sync.latch_columns` in
`assets/integrations/shopping.yaml`) only ever raised a row's tick: once Holon
saw an item ticked, a later pull that served it active again left Holon's
tick in place. The person's intent "buy this again" was lost.

## Root cause
`crates/holon-connections/src/reconcile.rs`, the latch loop of the merge arm,
copied a truthy peer value and ignored a falsy one. The reconciler kept no
record of the tick the peer had served before, so a peer's own un-tick could
not be told apart from a Holon tick the peer had not seen yet. Martin ruled
D103.a: lower the latch only for a real un-tick by the app.

## Missing piece
The keystone's remote-list alphabet had no "the app un-ticks an item"
transition, so no generated case reached the state.

## Remedy
The merge base (`remote_base`) records the peer's served value of every latch
column next to the merge columns. A latch lowers when the base says set and
the peer now serves it unset; a Holon tick the peer has not seen (base unset)
stays latched. New mutations `UntickRemotely` and `TickLocally` in the
keystone generator, replay cases
`remote-list-an-item-un-ticked-in-the-app-is-active-in-holon` (red before the
fix) and `remote-list-an-item-ticked-in-holon-stays-ticked-while-the-app-serves-it-active`
(the guard), and the reconcile PBT
`a_latch_column_lowers_only_on_the_peers_own_unset`.
