---
id: 2026-09-08-shopping-del-fixture-ages-out-of-the-tombstone-window
date: 2026-09-08
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `a_local_deletion_reaches_the_peer_as_a_del_command` hard-coded its tombstone
  at `2026-09-01T09:00:00Z` while the peer stamps the snapshot with the wall
  clock, so the fixture aged past the 7-day tombstone window and the test turned
  red on a calendar date with no product change behind it.
---

## Class

`FALSE-ALARM`: no product defect existed. The reconciler did exactly what it
promises — a tombstone older than the window stops suppressing the item, so the
item comes back and nothing is pushed. The fixture asserted "this deletion is
still live" with an absolute timestamp, which is a claim only true for one week.
Excluded from the four-class escape distribution.

## Bug

`holon-app::shopping_pull_mock a_local_deletion_reaches_the_peer_as_a_del_command`
fails at `crates/holon-app/tests/shopping_pull_mock.rs:544` with
`assertion left == right failed / left: 0 / right: 1` — no command was committed
to the mock peer. Found by the weave gate of lane `sharing-admit` at chain tip
`5644b6d3`; the same test passed at the wave-10 land gate on `main` `830d794f`,
which made it look like a regression from the iroh admission change.

It is not. The failure is a pure function of the wall clock and reproduces
identically on any tree from 2026-09-08T09:00:00Z onward.

## Root cause

The test set `gone.deleted_at = Some("2026-09-01T09:00:00Z")` and let the real
`RestShoppingPeer` build the snapshot, which stamps `fetched_at` with
`chrono::Utc::now()` (`crates/holon-app/src/shopping_rest.rs:101`).
`ShoppingReconciler::reconcile` measures the tombstone against that stamp
(`crates/holon-kitchen/src/shopping.rs:479,501-519`) and
`DEFAULT_TOMBSTONE_WINDOW_DAYS` is 7. From 2026-09-08T09:00:00Z the tombstone is
expired, so the branch taken is "the peer still lists the item, so it comes
back" — a `LocalIntent::Insert`, no `PushIntent::Remove`, nothing to commit.

The sibling reconciler tests in `crates/holon-kitchen/tests/shopping_reconcile.rs`
pass their own `FETCHED_AT` into `snapshot()`, so their identical date literals
are compared against a fixed clock and were never exposed.

## Missing piece

Nothing was un-covered by design; the test simply stopped exercising the leg it
names. The failure mode is a live test that silently degrades into an assertion
about the calendar — the harness had no way to state "a tombstone inside the
window" other than a literal, because the fetch time comes from production.

Worth noting for whoever closes
[[2026-09-02-deleting-a-shopping-item-is-undone-by-the-next-sync]]: that entry
names this test's shape as the place its closing test belongs, so the time bomb
would have silently weakened that coverage too.

## Remedy

FIXED, in `crates/holon-app/tests/shopping_pull_mock.rs`: the tombstone is now
derived from `chrono::Utc::now()` minus half of `DEFAULT_TOMBSTONE_WINDOW_DAYS`,
so "still live" is expressed against the same clock the peer stamps and stays
true if the constant changes.

Green: `cargo nextest run -p holon-app --test shopping_pull_mock` →
`11 tests run: 11 passed, 0 skipped` (`lane-logs/rev3-shopping-green-*.log`).
The preceding red at the same rev, before the fixture change, is in
`lane-logs/rev3-shopping-*.log` — same binary, same code, only the timestamp
differs, which is what isolates the cause to the fixture rather than to the
admission change woven at `5644b6d3`.

## Keystone repro

Not attempted. `general_e2e_composed_pbt` has no shopping-peer transition in its
catalog, and the defect is in a fixture rather than in production behaviour.
