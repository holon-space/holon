---
id: 2026-10-05-an-amount-authored-in-holon-is-nulled-by-the-next-sync
date: 2026-10-05
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A shopping amount authored in Holon ("2kg") was set to NULL by the same sync
  round that pushed the item, because the push sent no amount.
---

## Bug
Found by the verifier of the shopping free-text-amount lane (code audit, round 2).
A local shopping item with `count = "2kg"` and no watermark is pushed as an `add`.
The verifying re-pull of the same round returns the item with no amount, and the
reconciler writes `count = NULL` into the mirror.

## Root cause
`assets/integrations/shopping.yaml` commit `request` built `good: {name, cat, new: true}`
and left `count` out, although the peer accepts it
(docs/Plans/ThatShoppingList-API-2026-09-01.md, "Write (C2)"). `count` is a merge
column, so the reconciler (crates/holon-connections/src/reconcile.rs) applies the
peer's value — none — as a `SetColumn`. Red: lane-logs/r4-red-d1.RED.log
(`shopping_pull_mock::an_amount_authored_in_holon_reaches_the_peer_and_survives_the_round`:
left Null, right "2kg").

## Missing piece
Every test peer stored more than the real peer: the holon-app HTTP mock dropped the
amount on `add` but nothing asserted the mirror afterwards, and the generic
`FixtureListPeer` (crates/holon-connections-testing/src/peer.rs) stored the whole
command row instead of what the push mapping transmits. The keystone's remote-list
transition only changes the peer, so no keystone case ever pushes a local row.

## Remedy
Fixed: an `add` carries `count` verbatim. The mock stores what the command carries.
`FixtureListPeer` now runs the connection's declared `request` mapping and stores only
the transmitted row; the fixture table test asserts a pushed row's merge columns
survive the round (red when the fixture mapping drops `amount`:
lane-logs/r4-fixture-drops-amount.RED.log). The keystone's `RemoteListSync` gained an
`AuthorLocally` arm: a row created through the dispatcher, then pushed by the round.
The hand-authored case
`remote-list-local-amount-reaches-the-peer-and-a-duplicate-keeps-the-first` goes red
when the fixture `request` drops `amount` (`inv-remote-list-mirror-matches-ref`: mirror
amount "" against "2kg"; lane-logs/r5-red-d1-keystone.RED.log).
